//! `build_and_run_link` — the executable link driver extracted from
//! `link/mod.rs` to keep that file under the file-size gate (#5400 grew the
//! Windows response-file path past the 2000-line ceiling). This is a verbatim
//! move of the single ~1770-line orchestrator; all helpers it calls remain in
//! the parent `link` module and are pulled in via `use super::*`.

use super::*;

/// Construct the platform-specific linker command, append every required
/// argument (object files, libraries, frameworks, system libs, native libs,
/// geisterhand libs), invoke it, and bail on non-zero status.
///
/// Caller must have already handled the dylib output path; this function
/// only covers executable link. `args_input` is the user-supplied entry
/// `.ts` path (used for objcopy entry-stem matching on watchOS / visionOS /
/// iOS game-loop renames).
pub(crate) fn build_and_run_link(
    args_input: &Path,
    ctx: &CompilationContext,
    target: Option<&str>,
    obj_paths: &[PathBuf],
    obj_fingerprints: &[Option<String>],
    compiled_features: &[String],
    runtime_lib: &Path,
    stdlib_lib: &Option<PathBuf>,
    geisterhand_libs: Option<&super::super::geisterhand::GeisterhandLibs>,
    // #466 Phase 4 step 2: well-known native binding archives. Added
    // to the link line right after `stdlib_lib`. The matching
    // perry-stdlib feature was already stripped during the auto-
    // optimize rebuild, so the resulting link contains exactly one
    // copy of each `_js_<package>_*` symbol — no duplicates.
    well_known_libs: &[PathBuf],
    // No-auto / auto-fallback keeps the full prebuilt stdlib, so the
    // matching perry-stdlib feature was not stripped. Put wrappers first
    // in that shape so wrapper-only handles keep using their own surface
    // symbols instead of the bundled stdlib copies, but only after archive
    // preparation has removed unsafe duplicate allocator/runtime objects.
    prefer_well_known_before_stdlib: bool,
    // Issue #76 — `libperry_wasm_host.a` (wasmi-backed WebAssembly host
    // runtime). Only `Some(...)` when the user passed `--enable-wasm-runtime`
    // and the archive was located. Appended to the link command after the
    // stdlib block so the linker resolves `perry_wasm_host_*`
    // symbols referenced by `js_webassembly_*` shims in `perry-runtime`.
    wasm_host_lib: &Option<PathBuf>,
    exe_path: &Path,
    format: OutputFormat,
    // `--debug-symbols`: keep symbols / emit a PDB so RUST_BACKTRACE
    // panics in the compiled app symbolize. Windows-active today.
    // `--verbose` (#6899): print the exact linker invocation before spawning
    // it and its exit status after. The hang reported in #6899 sits between
    // the `[strip-dedup]` line and the `Wrote executable:` line — a window
    // that previously logged NOTHING even under `--verbose`, so a report
    // could not tell "linker never spawned" from "linker hung" from
    // "post-link step hung". These two lines split that window.
    verbose: u8,
) -> Result<LinkCacheStatus> {
    // #498 - supply-chain gate. Before any prebuilt archive hits the
    // linker, hash it and compare against `perry.lock`. First build
    // writes the lockfile; subsequent builds verify. Mismatch fails
    // with an actionable diagnostic (`perry lock --update <pkg>`).
    // `PERRY_LOCK_FROZEN=1` upgrades the verify to CI mode (refuses
    // to extend the lock); `PERRY_LOCK_UPDATE=<pkg>` deliberately
    // bumps the named package's hashes. Wired here so every backend
    // (LLVM / WASM / ArkTS / HarmonyOS / Glance / SwiftUI / JS)
    // inherits the gate from one chokepoint.
    super::super::run_lock_verify_for_compile(ctx, target)?;

    // Wear OS and every Android architecture share the NDK/cdylib link path.
    // Fully-static musl Linux target (#4826) — a sub-case of is_linux. The
    // link command itself is built in select_linker_command (musl driver +
    // `-static`); here it only changes which system libs we request.
    let is_musl = matches!(
        target,
        Some("linux-musl") | Some("linux-x86_64-musl") | Some("linux-aarch64-musl")
    );

    // The musl target is meant for headless/serverless binaries (Lambda,
    // scratch, distroless). The GTK4 UI backend (perry/ui) links the system
    // GTK/glib/webkit stack, which is only shipped for glibc and cannot be
    // statically linked into a musl binary. Fail fast with an actionable
    // message rather than emitting cryptic undefined-symbol errors (#4826).
    if is_musl && ctx.needs_ui {
        anyhow::bail!(
            "perry/ui is not supported with the static musl Linux target \
             (--libc musl / [linux] libc = \"musl\"): the GTK4 UI backend \
             requires dynamic glibc. Build the GUI app with the default \
             (glibc) Linux target, or drop perry/ui for a headless musl build."
        );
    }
    let is_windows = false;

    let mut cmd = select_linker_command(
        target,
    )?;

    // When ios-game-loop is enabled, rename _main to _perry_user_main in the
    // entry object file so the perry runtime's main() (from ios_game_loop.rs)
    // becomes the process entry point. It spawns _perry_user_main on a game thread.

    for obj_path in obj_paths {
        cmd.arg(obj_path);
    }
    cmd.args(super::link_pad_args());

    // HarmonyOS: pick up native C objects that build.rs scripts emitted
    // alongside the Rust artifacts. Rust's staticlib normally bundles these
    // into libperry_runtime.a, but on our macOS→OHOS cross-build the
    // `libmimalloc.a` wrapper ends up as a zero-member BSD-format archive
    // (BSD ar's `__.SYMDEF SORTED` layout — macOS-host `ar` creates it,
    // llvm-ar can't read it back), and rustc's "bundle native libs into
    // the staticlib" path silently skips it. Without us forwarding the
    // loose .o files to the final link, `libentry.so` ends up with
    // `mi_malloc_aligned` marked UND, and the OHOS dynamic linker rejects
    // dlopen with "symbol not found" at EntryAbility.onCreate time.
    //
    // We walk `target/<triple>/release/build/*/out/` and collect every
    // loose .o. This is coarser than Rust's per-crate link-lib directive
    // walking — it picks up .o files from any transitive C dep, not just
    // mimalloc — but that's a feature: the set is tiny in practice
    // (mimalloc is the only C dep in perry-runtime's closure today) and
    // any that turn out unreferenced are dead-stripped via --gc-sections.

    // Dead code stripping — safe because compile_init() emits func_addr
    // calls for every class method/getter during vtable registration. These
    // serve as linker roots that keep dynamically-dispatched methods alive.
    // Node-API addons resolve their C ABI from the executable itself. Keep
    // this exact export set narrow and entirely absent from ordinary builds.
    add_node_api_host_link_args(
        &mut cmd,
        ctx,
    )?;

    // `PERRY_LINK_LLD=1`: lld instead of GNU ld for a native glibc Linux
    // executable built on Linux with the default function layout
    // (`elf_linker`).
    let lld = super::elf_linker::use_lld(
        !is_musl
            && matches!(target, None | Some("linux"))
            && matches!(
                perry_codegen::program_function_layout(),
                perry_codegen::FunctionLayout::Default
            ),
    );
    {
        cmd.arg("-Wl,--gc-sections");
        // The runtime reads `__start_`/`__stop_perry_js_text` (the JS
        // text section's bounds, `error_stack_frames.rs`). By default a
        // reference to them retains every input section of that name,
        // which would keep every compiled JS function alive; with
        // start-stop-gc they retain nothing and unreferenced functions
        // are collected as before. GNU ld >= 2.37 and lld >= 13 accept
        // it; older linkers warn and ignore it.
        cmd.arg("-Wl,-z,start-stop-gc");
        // Native glibc Linux executables link at a fixed base (ET_EXEC),
        // as bun does. Every pointer in perry's static data (class method
        // slots, shape/key tables, string and closure headers, stack-map
        // function addresses) is then final at link time. As a PIE the
        // same pointers were relative relocations that ld.so applied at
        // every start: 1.74M of them for OpenCode, dirtying 32 MB of
        // `.data.rel.ro` copy-on-write before `main`.
        //
        // Why this is safe for the runtime: every image-address consumer
        // reads the load bias (`dl_iterate_phdr`'s `dlpi_addr`, `dladdr`)
        // instead of assuming one, and the fixed base (0x400000) sits
        // above the NaN-box handle band (`HANDLE_BAND_MAX` = 0x200000), so
        // a static object never classifies as a registry handle. Generated
        // and Rust objects stay PIC, and a PIC object links into a
        // fixed-base executable as is. Shared objects (plugins, Node-API
        // addons) are unaffected and still resolve the host's exports
        // dynamically. With no relative relocations left, DT_RELR
        // (`-z pack-relative-relocs`) has nothing to pack.
        //
        // Native glibc Linux only: not the static musl target, not a cross
        // link, not Android or HarmonyOS (those produce shared objects).
        if !is_musl
            && matches!(target, None | Some("linux"))
        {
            cmd.arg("-no-pie");
        }
        if let Some(mode) = lld {
            super::elf_linker::add_link_args(&mut cmd, mode);
        }
        // An Android program is a shared object, and a shared object
        // exports every default-visibility symbol: each one is a
        // --gc-sections root, so nothing in the runtime or stdlib is ever
        // collected, and references from adapters that only a linked
        // wrapper archive installs (perry-ext-http's `js_ext_http_*`,
        // `common::wrapper_imports`) stay as undefined symbols that make
        // `System.loadLibrary` fail. The Java side reaches the library
        // only through JNI, so it exports exactly that and everything
        // else is local, as in an executable. Patterns, not names: lld
        // rejects a named global the program doesn't define.
        // The runtime archives this link may pull from, for the default
        // hot-text order (`hot_text_order`).
        let stdlib_archive = stdlib_lib.as_deref().filter(|_| ctx.needs_stdlib);
        let runtime_archives: Vec<&Path> =
            std::iter::once(runtime_lib).chain(stdlib_archive).collect();
        // The script must fit the linker that runs: the native lld switch,
        // or a link line that picks lld itself (the cross links to Linux,
        // `PERRY_EXTRA_LINK_ARGS`, appended last below).
        let extra_args = super::extra_link_args();
        let runs_lld = lld.is_some()
            || super::elf_linker::driver_runs_lld(
                cmd.get_args()
                    .chain(extra_args.iter().map(std::ffi::OsStr::new)),
            );
        super::function_layout::add_link_args(
            &mut cmd,
            &ctx.cache_dir,
            &runtime_archives,
            runs_lld,
            // The default hot-data order: native glibc Linux executables,
            // where it was recorded and measured (`hot_data_order`).
            !is_musl
                && matches!(target, None | Some("linux")),
            verbose,
        )?;
        super::runtime_layouts::add_link_args(
            &mut cmd,
        );
        super::wrapper_hooks::add_link_args(
            &mut cmd,
            well_known_libs,
        );
        // PERRY_LINK_MAP=<path> — emit a linker map (which archive each symbol
        // resolves from) for diagnosing dup-symbol / shadowing bugs. Honor it on
        // every non-Windows linker, not just native macOS. GNU ld (ELF) spells
        // it `-Map=<file>`; ld64 (Apple) spells it `-map <file>`. The
        // cross-Apple linkers are driven directly (no `-Wl,` prefix).
        if let Some(map) = std::env::var_os("PERRY_LINK_MAP") {
            let map = map.to_string_lossy();
            cmd.arg(format!("-Wl,-Map,{map}"));
        }
    }

    // Link libraries - stdlib bundles perry-runtime; runtime provides base FFI symbols.
    // Note: libperry_stdlib.a may omit some runtime symbols (js_register_class_declaration,
    // js_register_class_name, etc.) due to Rust DCE on rlib dependencies. We always
    // link libperry_runtime.a as a fallback to fill these gaps. On macOS/Linux/ELF the
    // linker uses first-definition-wins for archives, so no duplicate symbol errors arise.
    // When UI lib is also linked, it bundles its own copy of perry-runtime.
    // For Android (ELF), skip the extra runtime when UI provides it.
    // On Windows (MSVC), always link the runtime — the UI lib's rlib dependency on
    // perry-runtime may not include all symbols (e.g., perry_init_guard_check_and_set).
    // watchOS: swiftc treats duplicate symbols as errors (not warnings like clang),
    // so skip the standalone runtime when the UI lib already bundles it.
    let mut source_well_known_libs = well_known_libs.to_vec();
    let mut seen_well_known = std::collections::HashSet::new();
    source_well_known_libs.retain(|path| seen_well_known.insert(path.clone()));
    // #9276: replacement evidence includes final-link reachability, not just
    // archive-index membership. ELF links are grouped to a fixed point
    // (#8930), Mach-O's linker already revisits archives, and COFF registers
    // lazy archive symbols globally. An unknown future linker stays
    // conservative and keeps the wrapper-local provider.
    let stdlib_resolves_retained_wrapper_refs = (ctx.needs_stdlib);
    let prepare_well_known = || {
        let cacheable = std::cell::Cell::new(true);
        let paths = source_well_known_libs
            .iter()
            .map(|wk| {
                // Wrappers precede stdlib here, so a wrapper's bundled
                // perry-runtime copy would win first-definition over stdlib's
                // and split the runtime's mutable globals in two (stdlib code
                // keeps its own copy via LTO-internal refs) — spawned async
                // tasks then starve because the event pump's wait-driver slot
                // is registered in one copy and read from the other. Drop the
                // bundled runtime members so stdlib's copy is the single
                // provider; the standalone runtime archive linked after
                // stdlib still fills any DCE gaps.
                let wk = match stdlib_lib {
                    Some(stdlib) => strip_bundled_runtime_from_well_known_lib(wk, stdlib)
                        .unwrap_or_else(|e| {
                            cacheable.set(false);
                            eprintln!(
                                "[strip-dedup] bundled-runtime drop skipped for {} (non-fatal): {e}",
                                wk.display()
                            );
                            // #5779: skipping this leaves two copies of perry-runtime's
                            // mutable globals; an in-process HTTP server + fetch program
                            // can then deadlock. It fails when no LLVM tool new enough to
                            // read the archives' bitcode is found — install the toolchain's
                            // llvm-tools (`rustup component add llvm-tools`) or a current LLVM.
                            eprintln!(
                                "[strip-dedup] warning: could not de-duplicate the bundled \
                                 perry-runtime — an in-process server+fetch program may \
                                 deadlock (#5779). Install `rustup component add llvm-tools` \
                                 or a current LLVM (e.g. `brew install llvm`)."
                            );
                            wk.clone()
                        }),
                    None => wk.clone(),
                };
                // Issue #5928: the runtime-specific drop above only targets
                // `perry_runtime-*` codegen units. Programs linking multiple
                // well-known libraries that each bundle a full "shared
                // tokio" HTTP stack (e.g. both `http` and `fastify`) still
                // collide on every OTHER shared transitive dependency
                // (tokio, hyper_util, h2, rustls, reqwest, ring, std, core,
                // …) — apply the general, fixed-point-safe dedup for those.
                let wk = match stdlib_lib {
                    Some(stdlib) => strip_bundled_shared_deps_from_well_known_lib(
                        &wk,
                        stdlib,
                        stdlib_resolves_retained_wrapper_refs,
                    )
                    .unwrap_or_else(|e| {
                        cacheable.set(false);
                        eprintln!(
                            "[strip-dedup] shared-deps drop skipped for {} (non-fatal): {e}",
                            wk.display()
                        );
                        wk.clone()
                    }),
                    None => wk.clone(),
                };
                strip_duplicate_objects_from_well_known_lib(&wk).unwrap_or_else(|error| {
                    cacheable.set(false);
                    eprintln!(
                        "[strip-dedup] wrapper symbol rewrite skipped for {} (non-fatal): {error}",
                        wk.display()
                    );
                    wk
                })
            })
            .collect();
        (paths, cacheable.get())
    };
    let (well_known_libs, wrappers_may_precede_stdlib): (Vec<PathBuf>, bool) =
        if prefer_well_known_before_stdlib && !source_well_known_libs.is_empty() {
            let prepared = prepare_well_known_archives(
                PreparedArchiveInputs {
                    cache_dir: &ctx.cache_dir,
                    target,
                    target_triple: rust_target_triple(target),
                    compiled_features,
                    runtime_lib,
                    stdlib_lib: stdlib_lib.as_deref(),
                    well_known_libs: &source_well_known_libs,
                },
                prepare_well_known,
            );
            (prepared.paths, prepared.safe_to_precede_stdlib)
        } else {
            (source_well_known_libs, true)
        };
    let wrappers_precede_stdlib = prefer_well_known_before_stdlib && wrappers_may_precede_stdlib;
    if prefer_well_known_before_stdlib && !wrappers_may_precede_stdlib {
        eprintln!(
            "[strip-dedup] warning: archive preparation was incomplete; linking the full stdlib before wrappers to keep its allocator and runtime authoritative"
        );
    }
    // Multiple specifiers can route to the same native archive (`http` and
    // `https` both select perry_ext_http). They were de-duplicated before the
    // archive-preparation pass so the expensive transform also runs once.
    // #8930: the archives below reference each other in BOTH directions and
    // ELF `ld` scans each once — bracket them (see `ElfArchiveGroup`).
    let archive_group = ElfArchiveGroup::open(
        &mut cmd,
        true,
        true,
    );
    if ctx.needs_stdlib {
        // On Windows/MSVC, always try to link stdlib because codegen unconditionally
        // declares all stdlib extern functions, creating import references that MSVC
        // won't dead-strip. On macOS/Linux, the linker ignores unreferenced archives.
        if let Some(ref stdlib) = stdlib_lib {
            // COFF archives must have one authoritative Rust dependency
            // graph. Put selected wrappers first, then let stdlib fill the
            // unresolved surface. The stdlib staticlib already bundles
            // perry-runtime, so Windows must not add it again.
            if wrappers_precede_stdlib {
                for wk in &well_known_libs {
                    cmd.arg(wk);
                }
            }
            // The stdlib's `external-*` adapters name wrapper symbols that
            // carry weak defaults on MSVC (perry-stdlib
            // `common::wrapper_imports`), and a weak reference does not
            // pull an archive member. Force each linked wrapper's own
            // object in through its install hook, so its definitions
            // always replace the defaults whatever else references it.
            // Tier-3 (tvOS/watchOS) std-duplication dedup; no-op elsewhere.
            cmd.arg(dedup_stdlib_for_tier3(target, stdlib));
            // #466 Phase 4 step 2: well-known bindings normally join the
            // link line right after perry-stdlib so they cover the exact
            // `_js_*` symbol gap that was just opened by stripping the
            // corresponding feature from the perry-stdlib rebuild.
            //
            // Non-Windows archive linkers retain the historical repeat for
            // stdlib bridge references. On Windows each wrapper appears
            // exactly once; if there was no wrapper-first pass, put it here.
            for wk in &well_known_libs {
                cmd.arg(wk);
            }
            // Also link runtime for symbols DCE'd from stdlib's bundled
            // perry-runtime; on tier-3 it's first stripped of stdlib's objects.
            // #5000 (macOS/Linux): the standalone runtime archive is built
            // WITHOUT the `stdlib` feature, so it also defines the no-op
            // stdlib_stubs (js_fetch_with_options, js_headers_new,
            // js_request_new, js_ws_*, js_readline_*). With ELF/Mach-O
            // first-definition-wins those stubs can satisfy the user's
            // fetch ref before perry-stdlib's real impls, so `fetch()`
            // no-ops and a program awaiting it hangs. When this build
            // actually uses stdlib (fetch / ws / readline), localize those
            // stub symbols in a copy of the runtime so perry-stdlib wins.
            let runtime_for_link = if ctx.uses_fetch || ctx.needs_stdlib {
                localize_stdlib_stub_symbols(runtime_lib, stdlib)
            } else {
                runtime_lib.to_path_buf()
            };
            cmd.arg(dedup_runtime_for_tier3(&runtime_for_link));
        } else {
            if ctx.needs_stdlib {
                eprintln!(
                    "Warning: stdlib required but {} not found, using runtime-only",
                    if is_windows {
                        "perry_stdlib.lib"
                    } else {
                        "libperry_stdlib.a"
                    }
                );
            }
            cmd.arg(runtime_lib);
        }
    } else {
        // Runtime-only linking — no stdlib needed.
        //
        // #5000 (Linux GTK4 UI): a bare UI program has
        // `ctx.needs_stdlib == false`, so perry-stdlib isn't linked above —
        // but the GTK4 UI branch below force-links it with
        // `--whole-archive --allow-multiple-definition` to satisfy glib
        // trampolines that call js_stdlib_process_pending /
        // js_promise_run_microtasks. With first-definition-wins, the
        // unlocalized runtime stubs linked here would shadow stdlib's real
        // impls, leaving those pumps as no-ops. Localize the runtime's stub
        // symbols first so stdlib wins, mirroring the needs_stdlib path.
        let force_stdlib_for_linux_ui =
            ctx.needs_ui && find_ui_library(target).is_some();
        let runtime_for_link = if force_stdlib_for_linux_ui {
            match stdlib_lib.clone().or_else(|| find_stdlib_library(target)) {
                Some(stdlib) => localize_stdlib_stub_symbols(runtime_lib, &stdlib),
                None => runtime_lib.to_path_buf(),
            }
        } else {
            runtime_lib.to_path_buf()
        };
        cmd.arg(&runtime_for_link);
    }
    // Optional archives can provide runtime-level services (for example the
    // component engine) without requiring perry-stdlib. The branches above
    // append them only when a stdlib archive is actually linked.
    let wrappers_linked_with_stdlib =
        (ctx.needs_stdlib) && stdlib_lib.is_some();
    if !wrappers_linked_with_stdlib {
        for library in &well_known_libs {
            cmd.arg(library);
        }
    }
    archive_group.close(&mut cmd);

    // Issue #76 — wasmi host runtime, opt-in via `--enable-wasm-runtime`.
    // Append after stdlib so the linker can resolve `perry_wasm_host_*`
    // symbols referenced by the always-present `js_webassembly_*` FFIs in
    // perry-runtime.
    if let Some(ref wasm_host) = wasm_host_lib {
        cmd.arg(wasm_host);
    }

    cmd.arg("-o").arg(exe_path).arg("-lc");

    // For plugin hosts, export symbols so dlopen'd plugins can resolve them.
    // Plugins are dylibs loaded via dlopen — they need to resolve:
    //   1. hone_host_api_* (plugin→host calls)
    //   2. js_*/perry_* (Perry runtime used by compiled plugin code)
    // We use -u to prevent dead_strip from removing these, keeping binary size small.
    if ctx.needs_plugins {
        {
            cmd.arg("-rdynamic");
        }
    }

    // Linux system libraries (cross-compile target)
    // Allow multiple definitions: stdlib bundles perry-runtime symbols,
    // and we also link perry-runtime directly for symbols DCE'd from stdlib.
    // macOS Mach-O uses first-definition-wins natively; ELF linkers need this flag.
    cmd.arg("-Wl,--allow-multiple-definition")
        .arg("-lm")
        .arg("-lpthread")
        .arg("-ldl");
    // No -lssl/-lcrypto: perry-stdlib bundles its own OpenSSL
    // (openssl-sys `vendored`) and TLS is rustls, so nothing links the
    // system libraries and users need no OpenSSL dev package.

    // Issue #607 — watchOS targets always link the UI lib regardless of
    // `ctx.needs_ui`. The watchOS Swift template (`PerryWatchApp.swift`)
    // unconditionally references four `@_silgen_name`'d Rust symbols
    // (`perry_watchos_tree_version` / `perry_watchos_toggle_changed` /
    // `perry_watchos_toast_seq` / `perry_watchos_toast_dismiss`) that
    // live in `libperry_ui_watchos.a`. A console-only TS program has
    // `needs_ui = false`, so the UI lib was previously not added to the
    // link line — leaving those four symbols undefined and the link
    // failing. Forcing the UI lib for watchOS adds ~MBs but unblocks
    // `console.log("ok")`-only programs from compiling.
    if ctx.needs_ui {
        // When geisterhand is enabled, prefer the geisterhand-enabled UI lib
        // (it contains widget registration calls that the normal lib doesn't have)
        let ui_lib_option = if let Some(libs) = geisterhand_libs {
            Some(libs.ui.clone())
        } else {
            find_ui_library(target)
        };
        if let Some(ui_lib) = ui_lib_option {
            // The UI staticlib bundles perry_runtime + Rust std. When perry-stdlib
            // is also linked (which bundles the same), duplicate symbols cause
            // crashes (conflicting static state initialization). Strip fully
            // duplicated members by symbol-set evidence on Apple and Windows;
            // members with UI-specific monomorphizations are retained. Android
            // remains untouched because skip_runtime=true makes the UI lib the
            // sole provider of perry-runtime symbols.
            let ui_lib = {
                // #8455: the trim's evidence set must equal the archives that
                // will actually be on this link line. stdlib joins it when
                // `ctx.needs_stdlib || is_windows` (the arm above), and on
                // Linux unconditionally (appended after the UI lib below). A
                // pure-UI ld64 program links neither — counting stdlib's
                // symbols anyway dropped the UI lib's bundled-std members that
                // the thin-LTO runtime archive does not export (e.g.
                // `<Stdout as Write>::flush` from `PerryTestExitTarget::
                // test_exit`), failing every UI doc-test at link.
                let stdlib_on_link_line: Option<PathBuf> = stdlib_lib.clone().or_else(|| find_stdlib_library(target));
                let stdlib_evidence = match stdlib_on_link_line {
                    Some(ref path) => StdlibEvidence::Linked(path.as_path()),
                    None => StdlibEvidence::NotLinked,
                };
                let trimmed = match strip_duplicate_objects_from_lib_with_evidence(
                    &ui_lib,
                    stdlib_evidence,
                ) {
                    Ok(trimmed) => trimmed,
                    Err(e) => {
                        eprintln!("[strip-dedup] skipped for UI lib (non-fatal): {e}");
                        ui_lib
                    }
                };
                // #5920 follow-up: the prebuilt UI lib bundles dependency
                // copies (perry-runtime, std, itoa, …) from a foreign crate
                // graph, so the member-name trim above can never drop them
                // against a locally rebuilt (auto-optimized) stdlib. On Mach-O
                // both copies then load and ld64.lld fails with duplicate
                // `_js_*` / std externs. Localize every bundled global that
                // the actually-linked stdlib/runtime provide, so UI references
                // rebind to the single linked copy. Linux tolerates the
                // duplicates via --allow-multiple-definition, so restrict to
                // the ld64 shapes.
                trimmed
            };
            cmd.arg(&ui_lib);

            {
                // Allow multiple definitions from perry-runtime in both stdlib and UI lib
                cmd.arg("-Wl,--allow-multiple-definition");
                // libperry_ui_gtk4.a's glib::source::trampoline_local
                // closures call perry-stdlib's js_stdlib_process_pending /
                // js_promise_run_microtasks. When ctx.needs_stdlib is false
                // (bare UI program), stdlib isn't linked via the earlier
                // path — add it here, AFTER the UI lib, so ld pulls the
                // real pump implementations for exactly those references.
                // --allow-multiple-definition above lets its bundled
                // perry-runtime copy coexist with libperry_runtime.a. The
                // async-bridge feature is force-enabled for UI builds (see
                // build_optimized_libs), so the real js_stdlib_process_pending
                // is guaranteed present in libperry_stdlib.a. (When
                // ctx.needs_stdlib is true, stdlib already sits BEFORE the
                // UI lib on the link line; this second occurrence resolves
                // the UI trampoline references that weren't yet undefined
                // during the first left-to-right archive scan.)
                //
                // Demand-driven (plain archive) linking, NOT
                // `--whole-archive` (#6029): the prebuilt full stdlib
                // bundles its Rust deps' native C archives (aws-lc via
                // reqwest/rustls, sqlite, brotli, …), and force-including
                // every member also pulls objects nothing references.
                // aws-lc's `hrss.o` calls `poly_Rq_mul`, whose defining
                // assembly object (`hrss/asm/poly_rq_mul.S`) is missing
                // from aws-lc-sys's cc-builder file list on linux x86_64,
                // so `--whole-archive` turned that latent upstream gap into
                // a hard `undefined reference to aws_lc_0_41_0_poly_Rq_mul`
                // for every GUI app linked against the prebuilt stdlib —
                // and, pre-#5983, likewise dragged in the stdlib's
                // `js_ext_http_*` references with perry-ext-http nowhere on
                // the link line. Demand-driven pulling never loads those
                // members. It is sound for the pumps because the runtime
                // archive linked earlier had its no-op stdlib stubs
                // localized (see force_stdlib_for_linux_ui / #5000): the
                // UI lib's pump references stay undefined until this
                // archive is scanned, so ld takes them from perry-stdlib.
                // (That localization was load-bearing under --whole-archive
                // too — with first-definition-wins, a force-included copy
                // never outranked an unlocalized runtime stub.)
                let linux_stdlib_for_ui =
                    stdlib_lib.clone().or_else(|| find_stdlib_library(target));
                if let Some(ref stdlib) = linux_stdlib_for_ui {
                    cmd.arg(stdlib);
                }
                // GTK4 / PulseAudio / GStreamer / libshumate / WebKitGTK
                // system libs, each via pkg-config with a hardcoded fallback
                // (#181, #423, #517, #658) — see linux_ui_libs.rs.
                linux_ui_libs::add_linux_ui_system_libs(&mut cmd);
            }

            match format {
                OutputFormat::Text => {
                    println!("Linking perry/ui (native UI) from {}", ui_lib.display())
                }
                OutputFormat::Json => {}
            }
        } else {
            let windows_rust_triple = windows_target_arch()
                .map(WindowsTargetArch::rust_triple)
                .unwrap_or("x86_64-pc-windows-msvc");
            let windows_ui_build_cmd =
                format!("cargo build --release -p perry-ui-windows --target {windows_rust_triple}");
            let windows_winui_build_cmd = format!(
                "cargo build --release -p perry-ui-windows-winui --target {windows_rust_triple}"
            );
            let (lib_name, build_cmd) = (
                "libperry_ui_gtk4.a",
                "cargo build --release -p perry-ui-gtk4 --target x86_64-unknown-linux-gnu",
            );
            return Err(anyhow!(
                "perry/ui imported but {} not found. Build with: {}",
                lib_name,
                build_cmd
            ));
        }
    }

    // Reuse the prepared set; rediscovery can select a different runtime.
    if let Some(libs) = geisterhand_libs {
        cmd.arg(&libs.server);
        cmd.arg(runtime_lib);
        cmd.arg("-Wl,--allow-multiple-definition");
        // On Windows, force-include geisterhand registry symbols from stdlib.
        // lld-link scans archives left-to-right once, so the stdlib's
        // geisterhand objects are skipped on first scan (no references yet).
        // /INCLUDE forces the linker to pull in the specific symbols.
        match format {
            OutputFormat::Text => println!("Linking geisterhand (in-process fuzzer)"),
            OutputFormat::Json => {}
        }
    }

    // Build and link external native libraries from perry.nativeLibrary manifests.
    // Swift sources are deduplicated across the loop — modules sharing the same
    // package.json all see the same swift_sources entries, but each file should
    // be compiled + linked once. Without this, swift's mangled symbols for
    // structs/classes duplicate N times.
    let mut seen_swift_sources: std::collections::HashSet<PathBuf> =
        std::collections::HashSet::new();
    for native_lib in &ctx.native_libraries {
        if let Some(ref target_config) = native_lib.target_config {
            if !target_config.available {
                if let (OutputFormat::Text, Some(reason)) =
                    (format, target_config.unavailable_reason.as_deref())
                {
                    println!(
                        "Skipping native library {} for this target: {}",
                        native_lib.module, reason
                    );
                }
                continue;
            }

            match format {
                OutputFormat::Text => {
                    println!("Building native library: {} ...", native_lib.module)
                }
                OutputFormat::Json => {}
            }

            // Issue #860 — prebuilt-distribution shortcut. When the
            // wrapper's manifest specified a `prebuilt:` path that
            // resolved to an on-disk static library, skip the cargo
            // build entirely and link the prebuilt archive directly.
            // `frameworks` / `libs` / `pkgConfig` / `lib_dirs` are
            // still honored below — those are linker flags the host
            // toolchain needs regardless of where the `.a` came from.
            if let Some(prebuilt) = target_config.prebuilt.as_ref() {
                if !prebuilt.exists() {
                    return Err(anyhow!(
                        "Prebuilt native library declared by {} not found at {}. \
                         If this package is distributed via npm `optionalDependencies` \
                         (esbuild/sharp pattern), make sure the per-platform subpackage \
                         is installed for the current host/target.",
                        native_lib.module,
                        prebuilt.display()
                    ));
                }
                cmd.arg(prebuilt);
                match format {
                    OutputFormat::Text => {
                        println!("Linking prebuilt native library: {}", prebuilt.display())
                    }
                    OutputFormat::Json => {}
                }
            } else {
                // Build the Rust crate
                let cargo_toml = target_config.crate_path.join("Cargo.toml");
                if cargo_toml.exists() {
                    // Tier 3 targets (tvOS, watchOS) need nightly + build-std

                    // #505: optionally wrap the cargo invocation in
                    // `sandbox-exec` on macOS to deny network and
                    // restrict FS writes during `build.rs` execution.
                    // Off by default for backwards compat; opted into
                    // via `PERRY_SANDBOX_BUILDRS=1`. Packages listed
                    // in `perry.allowUnsandboxedBuild` are exempt.
                    let mut cargo_cmd =
                        super::super::sandbox_buildrs::wrap_cargo_command(ctx, &native_lib.module);
                    cargo_cmd
                        .arg("build")
                        .arg("--release")
                        .arg("--manifest-path")
                        .arg(&cargo_toml);

                    // perry.toml `[native-library."<pkg>"]` feature
                    // forwarding — see native_features.rs.
                    native_features::apply_native_library_override(
                        &mut cargo_cmd,
                        &ctx.project_root,
                        &native_lib.module,
                        matches!(format, OutputFormat::Text),
                    );

                    if let Some(triple) = rust_target_triple(target) {
                        cargo_cmd.arg("--target").arg(triple);
                    }


                    // For Android, ensure 16 KB page size alignment (required by Google Play),
                    // and force the global-dynamic TLS model (#1529): the native lib's TLS
                    // relocations are baked into the dlopen'd `libperry_app.so`, so an
                    // Initial-Executable model (rustc's android default) crashes at load.

                    // For HarmonyOS, point cargo at the OHOS SDK's clang + sysroot
                    // so cc-rs and rustc's linker invocation actually use the
                    // cross-toolchain instead of falling back to the host `cc`.
                    // #1508: For Android, do the same with the NDK so cc-rs can
                    // compile native C deps (libsqlite3-sys / libmimalloc-sys
                    // / etc.) using the NDK clang. Without this, cc-rs falls
                    // back to the host `cc` and fails with
                    // `failed to find tool "clang.exe"` on Windows (and
                    // architecturally-mismatched objects on Unix).

                    // #1303 — when the wrapper crate's `build.rs` gates an
                    // optional vendored SDK on an env var (`frameworks_env`)
                    // and the dev declared a project-relative `framework_dir`
                    // in `perry.toml [google_auth]`, export the resolved
                    // absolute path so `build.rs` opts the real SDK in. This
                    // fires on the local machine AND on the `perry publish`
                    // worker (where the dev's shell env doesn't transfer, but
                    // the uploaded perry.toml + dir do). If the env var is
                    // already set we re-export the same value — idempotent.
                    if let Some(env_name) = target_config.frameworks_env.as_deref() {
                        if let Some(dir) = resolve_optional_framework_dir(env_name, args_input) {
                            if dir.is_dir() {
                                cargo_cmd.env(env_name, &dir);
                            }
                        }
                    }

                    let cargo_status =
                        super::super::tool_output::run_internal_tool(&mut cargo_cmd, verbose)?;
                    if !cargo_status.success() {
                        return Err(anyhow!(
                            "Failed to build native library crate for {}: {}",
                            native_lib.module,
                            target_config.crate_path.display()
                        ));
                    }
                }

                // Find and link the static library
                let lib_name = &target_config.lib_name;
                if !lib_name.is_empty() {
                    // Search in the crate's target directory first, then standard paths.
                    // Refs #564: probe both `target/release/` and
                    // `target/<host-triple>/release/` for native builds — cargo
                    // writes to the triple-prefixed dir when a default target is
                    // pinned via `[build] target` / `CARGO_BUILD_TARGET` /
                    // `rust-toolchain.toml`.
                    let crate_target_dir = target_config.crate_path.join("target");
                    let lib_path = super::super::library_search::locate_native_lib_artifact(
                        &crate_target_dir,
                        target,
                        lib_name,
                    );

                    if let Some(lib) = lib_path {
                        let lib = dedup_native_lib_for_tier3(lib);
                        // For shared libraries (.so) on Android, use -L/-l so the linker
                        // records just the soname (not the full build path) in DT_NEEDED.
                        {
                            // When building a plugin host on macOS, force-load plugin-related native
                            // libraries so their symbols are available for dlopen'd plugin dylibs.
                            cmd.arg(&lib);
                        }
                        match format {
                            OutputFormat::Text => {
                                println!("Linking native library: {}", lib.display())
                            }
                            OutputFormat::Json => {}
                        }
                    } else {
                        return Err(anyhow!(
                            "Native library {} not found after building {} crate",
                            lib_name,
                            native_lib.module
                        ));
                    }
                }
            } // closes else of `if let Some(prebuilt) = ...` (issue #860)

            // Add platform frameworks
            for framework in &target_config.frameworks {
                cmd.arg("-framework").arg(framework);
            }

            // Issue #1304 — vendored-SDK frameworks (e.g. GoogleSignIn
            // for `@perryts/google-auth`). These live in a directory the
            // app dev built/downloaded locally, named by the wrapper's
            // `frameworks_env` env var. When that var is set and resolves
            // to an existing directory, add it as a framework search path
            // (`-F <dir>`) and emit one `-framework <name>` per declared
            // `optional_frameworks` entry. When it's unset (or points at
            // something that isn't a directory) we skip silently: the
            // wrapper's `#if canImport(...)` Swift bridge already compiles
            // a no-SDK fallback path, so the binary still links and
            // returns a runtime "framework not linked" result rather than
            // failing with undefined `GID*` symbols.
            //
            // Contract is static frameworks only — `-framework` links the
            // archive directly with no `.app/Frameworks/` embed + rpath.
            //
            // #1303 — the search dir resolves from the `frameworks_env` env
            // var (local) or, when unset, the project-relative
            // `perry.toml [google_auth].framework_dir` (so a `perry publish`
            // worker build links the real SDK instead of the stub).
            if let Some(env_name) = target_config.frameworks_env.as_deref() {
                if !target_config.optional_frameworks.is_empty() {
                    match resolve_optional_framework_dir(env_name, args_input) {
                        Some(dir) if dir.is_dir() => {
                            cmd.arg("-F").arg(&dir);
                            for framework in &target_config.optional_frameworks {
                                cmd.arg("-framework").arg(framework);
                            }
                            if let OutputFormat::Text = format {
                                println!(
                                    "Linking {} optional framework(s) for {} ({})",
                                    target_config.optional_frameworks.len(),
                                    native_lib.module,
                                    dir.display()
                                );
                            }
                        }
                        Some(dir) => {
                            if let OutputFormat::Text = format {
                                println!(
                                    "Skipping optional frameworks for {}: {:?} is not a directory",
                                    native_lib.module,
                                    dir.display()
                                );
                            }
                        }
                        None => {
                            // Neither env var nor framework_dir → silent skip
                            // (the wrapper's canImport fallback keeps linking).
                        }
                    }
                }
            }

            // Add library search paths. MSVC link.exe takes `/LIBPATH:`;
            // every other linker we drive (clang/ld on Apple, gcc/ld on
            // Linux/Android/HarmonyOS) understands `-L`. Mirror the
            // `target_config.libs` branch immediately below so a
            // `targets.windows.libDirs` entry actually resolves the
            // `{lib}.lib` lookups instead of being a silent no-op.
            for lib_dir in &target_config.lib_dirs {
                cmd.arg(format!("-L{}", lib_dir.display()));
            }

            // Add platform libraries
            for lib in &target_config.libs {
                cmd.arg(format!("-l{}", lib));
            }

            // Add pkg-config libraries
            for pkg in &target_config.pkg_config {
                pkg_config::validate_pkg_config_name(pkg)?;
                if let Ok(output) = Command::new("pkg-config").args(["--libs", pkg]).output() {
                    if output.status.success() {
                        let libs = String::from_utf8_lossy(&output.stdout);
                        for flag in libs.split_whitespace() {
                            cmd.arg(flag);
                        }
                    }
                }
            }

            for backend in &target_config.backends {
                if !backend.available {
                    if let (OutputFormat::Text, Some(reason)) =
                        (format, backend.unavailable_reason.as_deref())
                    {
                        println!(
                            "Skipping {} backend for {}: {}",
                            backend.backend.as_str(),
                            native_lib.module,
                            reason
                        );
                    }
                }
            }

            for backend in select_available_backend_link_metadata(target_config) {
                if let Some(prebuilt) = backend.prebuilt.as_ref() {
                    if !prebuilt.exists() {
                        return Err(anyhow!(
                            "Prebuilt {} backend library declared by {} not found at {}. \
                             Install the matching optional dependency or update \
                             perry.nativeLibrary.targets.<target>.backends.{}.prebuilt.",
                            backend.backend.as_str(),
                            native_lib.module,
                            prebuilt.display(),
                            backend.backend.as_str()
                        ));
                    }
                    cmd.arg(prebuilt);
                    match format {
                        OutputFormat::Text => println!(
                            "Linking prebuilt {} backend library: {}",
                            backend.backend.as_str(),
                            prebuilt.display()
                        ),
                        OutputFormat::Json => {}
                    }
                }

                for framework in &backend.frameworks {
                    cmd.arg("-framework").arg(framework);
                }
                for lib_dir in &backend.lib_dirs {
                    cmd.arg(format!("-L{}", lib_dir.display()));
                }
                for lib in &backend.libs {
                    cmd.arg(format!("-l{}", lib));
                }
                for pkg in &backend.pkg_config {
                    pkg_config::validate_pkg_config_name(pkg)?;
                    if let Ok(output) = Command::new("pkg-config").args(["--libs", pkg]).output() {
                        if output.status.success() {
                            let libs = String::from_utf8_lossy(&output.stdout);
                            for flag in libs.split_whitespace() {
                                cmd.arg(flag);
                            }
                        }
                    }
                }
            }

            // Issue #5812 item 2 — auto-append the Windows system libs that
            // wgpu's graphics backends reference but cargo's `#[link]` attrs
            // on the wgpu crate don't cover (d3dcompiler / opengl32). Without
            // them a Windows app linking a wgpu-backed native ext failed with
            // LNK2019 unresolved externals (the reporter patched the link
            // line by hand). See `windows_wgpu_backend_syslibs`.

            // Compile manifest-declared Swift sources to object files and
            // append them to the link line. Used by `--features watchos-swift-app`
            // so a native lib can ship its own `@main struct App: App`.
            if !target_config.swift_sources.is_empty() {
                return Err(anyhow!(
                    "perry.nativeLibrary.targets.<target>.swift_sources is only supported on watchos/watchos-simulator"
                ));
            }

            // Metal sources are compiled + packed into <app>.app/default.metallib
            // after the `.app` bundle is created below. Just validate the target
            // here so we fail early with a clear message instead of silently
            // dropping shaders on non-Apple-bundle targets.
            if !target_config.metal_sources.is_empty()
            {
                return Err(anyhow!(
                    "perry.nativeLibrary.targets.<target>.metal_sources is only supported on ios / ios-simulator / tvos / tvos-simulator / watchos / watchos-simulator / visionos / visionos-simulator"
                ));
            }
        }
    }

    // macOS privacy APIs (including camera/microphone requests made by
    // WKWebView) consult the process Info.plist for usage-description keys.
    // Keep the linked binary's embedded metadata identical to its macOS app
    // bundle metadata; the standalone executable remains available as well.
    // Without this, WKWebView media capture can be denied by the platform even
    // when WKUIDelegate grants the web-origin permission.
    let mut embedded_info_plist_path: Option<PathBuf> = None;

    let link_cache_status = prepare_link_cache_status(
        &ctx.cache_dir,
        target,
        &cmd,
        obj_paths,
        obj_fingerprints,
        exe_path,
    );
    if !link_cache_status.linked {
        if let Some(path) = embedded_info_plist_path {
            let _ = fs::remove_file(path);
        }
        winui_assets::deploy_winui_runtime_assets()?;
        return Ok(link_cache_status);
    }

    // Windows hosts cap a spawned process's command line (`CreateProcess` ~32
    // KiB); a link with many object files — ≥~450 local modules (e.g. dense
    // barrel `export *` re-exports, or the `@earendil-works/pi-ai` module
    // graph) — overflows it and fails with `The filename or extension is too
    // long. (os error 206)` before the linker even starts. Route the whole
    // argument vector through a linker *response file* (`@file`), which
    // `link.exe` / `lld-link` (and `clang`/`cc`) read instead of the command
    // line, so the invocation no longer scales with module count. Other hosts
    // take the same path once the arguments are large: macOS caps them at
    // 1 MiB (`link_args_need_response_file`).
    // `PERRY_FORCE_LINK_RESPONSE_FILE=1` forces the path for testing.
    cmd.args(super::extra_link_args());

    let mut response_file_to_clean: Option<PathBuf> = None;
    if std::env::var_os("PERRY_FORCE_LINK_RESPONSE_FILE").is_some()
        || link_args_need_response_file(cmd.get_args())
    {
        // MSVC-style quoting for the native-Windows linker (link.exe/lld-link);
        // GNU-style for a clang/cc driver (the forced-test path on macOS/Linux).
        if let Some((rsp_cmd, rsp_path)) = rewrite_link_with_response_file(&cmd) {
            cmd = rsp_cmd;
            response_file_to_clean = Some(rsp_path);
        }
    }

    // #6899 breadcrumbs: everything between `[strip-dedup]` and
    // `Wrote executable:` used to run silently, so a hang in this window was
    // unattributable. Print the spawn and the return, on stderr like the
    // strip-dedup lines, flushed via eprintln's line buffering.
    if verbose > 0 {
        let program = cmd.get_program().to_string_lossy().into_owned();
        let rendered: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        eprintln!("[link] invoking: {} {}", program, rendered.join(" "));
    }
    let status_result = super::super::tool_output::run_internal_tool(&mut cmd, verbose);
    if verbose > 0 {
        match &status_result {
            Ok(status) => eprintln!("[link] linker exited: {status}"),
            Err(e) => eprintln!("[link] linker spawn failed: {e}"),
        }
    }
    if let Some(path) = response_file_to_clean {
        let _ = fs::remove_file(path);
    }
    if let Some(path) = embedded_info_plist_path {
        let _ = fs::remove_file(path);
    }
    let status = status_result?;

    if !status.success() {
        return Err(anyhow!("Linking failed"));
    }

    winui_assets::deploy_winui_runtime_assets()?;

    Ok(link_cache_status)
}


#[cfg(test)]
mod workspace_build_std_tests {
}

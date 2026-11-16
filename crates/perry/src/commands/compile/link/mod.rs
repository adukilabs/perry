//! Build and run the executable link command.
//!
//! Tier 2.1 final extraction (v0.5.342) — moves the per-platform link command
//! construction out of `crates/perry/src/commands/compile.rs::run_with_parse_cache`.
//! Pre-extraction, the link logic was a ~1240-LOC inline block inside the
//! orchestrator, fanning out across macOS / iOS / tvOS / visionOS / watchOS /
//! Android / Linux / Windows / cross-compile permutations. Co-locating it here
//! lets the orchestrator stay focused on parse / lower / codegen / cache / link
//! sequencing instead of churning the same file every time a platform-specific
//! link flag changes.
//!
//! The `dylib` link path stays inline in compile.rs because it returns early
//! with a `CompileResult`. Per-platform `.app` bundling and Android companion
//! `.so` copying also stay in compile.rs — they happen after the link
//! returns and need access to many post-link variables (`exe_path`,
//! `result_bundle_id`, etc.) that don't belong in this module.
//!
//! Wave 4 split (v0.5.x) — the per-platform linker-`Command` selection (~615
//! LOC of `if is_watchos { swiftc … } else if is_ios { clang … } else if …`)
//! moved into `platform_cmd.rs` so this file stays under the 2k-LOC soft
//! ceiling. The orchestrator below still drives the full link line; only the
//! initial `Command::new(<toolchain>)` + sysroot/triple/entry-rewrite prelude
//! lives in the sibling module.

use anyhow::{anyhow, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::OutputFormat;

use super::{
    dedup_native_lib_for_tier3, dedup_runtime_for_tier3,
    dedup_stdlib_for_tier3, find_llvm_tool,
    find_stdlib_library,
    find_ui_library, localize_stdlib_stub_symbols, rust_target_triple,
    strip_bundled_runtime_from_well_known_lib, strip_bundled_shared_deps_from_well_known_lib,
    strip_duplicate_objects_from_lib_with_evidence, strip_duplicate_objects_from_well_known_lib,
    windows_target_arch, CompilationContext,
    StdlibEvidence, WindowsTargetArch,
};

mod archive_cache;
mod build_and_run;
mod elf_linker;
mod function_layout;
mod glibc_version;
mod hot_data_order;
mod hot_text_order;
mod hot_text_order_macho;
mod link_cache;
mod linux_dylib_libs;
mod linux_ui_libs;
mod native_features;
mod pkg_config;
mod platform_cmd;
mod runtime_layouts;
mod windows_link;
mod winui_assets;
mod wrapper_hooks;

use archive_cache::{prepare_well_known_archives, PreparedArchiveInputs};
pub(super) use build_and_run::build_and_run_link;
use link_cache::prepare_link_cache_status;
pub(super) use link_cache::{write_link_cache_manifest, LinkCacheStatus};
pub(super) use linux_dylib_libs::push_unix_dylib_output;
pub use platform_cmd::select_linker_command;
 // consumed only by windows_link_tests
 // windows_link_tests (#6023)



fn add_node_api_host_link_args(
    cmd: &mut Command,
    ctx: &CompilationContext,
) -> Result<()> {
    // Export the addons' Node-API imports and Perry's supplemental C surface.
    // An open addon can still reach the full Node-API host.
    let mut exports = ctx.node_api_host_exports();
    exports.extend(ctx.perry_c_host_exports());
    if exports.is_empty() {
        return Ok(());
    }
    for symbol in &exports {
        cmd.arg(format!("-Wl,-u,{symbol}"));
        cmd.arg(format!("-Wl,--export-dynamic-symbol={symbol}"));
    }
    Ok(())
}

#[cfg(test)]
mod node_api_symbol_inventory_tests {
    use perry_runtime_manifest::{node_api_host_symbols, perry_c_host_symbols};
    use std::collections::BTreeSet;

    #[test]
    fn checked_in_inventory_matches_every_exported_host_function() {
        let source = [
            include_str!("../../../../../perry-runtime/src/node_api_host/async_work.rs"),
            include_str!("../../../../../perry-runtime/src/node_api_host/bigint.rs"),
            include_str!("../../../../../perry-runtime/src/node_api_host/buffers.rs"),
            include_str!("../../../../../perry-runtime/src/node_api_host/functions.rs"),
            include_str!("../../../../../perry-runtime/src/node_api_host/lifecycle.rs"),
            include_str!("../../../../../perry-runtime/src/node_api_host/loader.rs"),
            include_str!("../../../../../perry-runtime/src/node_api_host/metadata.rs"),
            include_str!("../../../../../perry-runtime/src/node_api_host/mod.rs"),
            include_str!("../../../../../perry-runtime/src/node_api_host/modules.rs"),
            include_str!("../../../../../perry-runtime/src/node_api_host/promises.rs"),
            include_str!("../../../../../perry-runtime/src/node_api_host/properties.rs"),
            include_str!("../../../../../perry-runtime/src/node_api_host/scopes.rs"),
            include_str!("../../../../../perry-runtime/src/node_api_host/tsfn.rs"),
            include_str!("../../../../../perry-runtime/src/node_api_host/values.rs"),
        ]
        .join("\n");
        let export_re = perry_perex::tooling::Regex::new(
            r#"(?s)#\[no_mangle\]\s*pub\s+(?:unsafe\s+)?extern\s+"C"\s+fn\s+((?:napi|node_api)_[A-Za-z0-9_]+)"#,
        )
        .unwrap();
        let source_exports = export_re
            .captures_iter(&source)
            .map(|capture| capture[1].to_string())
            .collect::<BTreeSet<_>>();
        let inventory = node_api_host_symbols()
            .map(str::to_string)
            .collect::<BTreeSet<_>>();
        assert_eq!(inventory, source_exports);
    }

    #[test]
    fn perry_c_inventory_matches_exported_runtime_functions() {
        let source = include_str!("../../../../../perry-runtime/src/embedded.rs");
        let export_re = perry_perex::tooling::Regex::new(
            r#"(?s)#\[no_mangle\]\s*pub\s+(?:unsafe\s+)?extern\s+"C"\s+fn\s+(perry_[A-Za-z0-9_]+)"#,
        )
        .unwrap();
        let source_exports = export_re
            .captures_iter(source)
            .map(|capture| capture[1].to_string())
            .collect::<BTreeSet<_>>();
        let inventory = perry_c_host_symbols()
            .map(str::to_string)
            .collect::<BTreeSet<_>>();
        assert_eq!(inventory, source_exports);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeBackendLinkMetadata {
    backend: super::NativeBackend,
    prebuilt: Option<PathBuf>,
    frameworks: Vec<String>,
    libs: Vec<String>,
    lib_dirs: Vec<PathBuf>,
    pkg_config: Vec<String>,
}

fn select_available_backend_link_metadata(
    target_config: &super::TargetNativeConfig,
) -> Vec<NativeBackendLinkMetadata> {
    if !target_config.available {
        return Vec::new();
    }

    target_config
        .backends
        .iter()
        .filter(|backend| backend.available)
        .map(|backend| NativeBackendLinkMetadata {
            backend: backend.backend,
            prebuilt: backend.prebuilt.clone(),
            frameworks: backend.frameworks.clone(),
            libs: backend.libs.clone(),
            lib_dirs: backend.lib_dirs.clone(),
            pkg_config: backend.pkg_config.clone(),
        })
        .collect()
}


#[cfg(test)]
mod native_package_selection_tests {
    use super::*;
    use crate::commands::compile::{
        NativeBackend, NativeBackendConfig, NativeBackendPackageMetadata, TargetNativeConfig,
    };

    fn target_config() -> TargetNativeConfig {
        TargetNativeConfig {
            available: true,
            unavailable_reason: None,
            crate_path: PathBuf::from("crate"),
            lib_name: "demo".to_string(),
            prebuilt: Some(PathBuf::from("target/libdemo.a")),
            frameworks: vec!["Metal".to_string()],
            optional_frameworks: Vec::new(),
            frameworks_env: None,
            libs: vec!["z".to_string()],
            lib_dirs: vec![PathBuf::from("vendor/lib")],
            pkg_config: vec!["openssl".to_string()],
            resources: vec![PathBuf::from("resources/common.dat")],
            shader_outputs: vec![PathBuf::from("shaders/common.spv")],
            backends: Vec::new(),
            swift_sources: Vec::new(),
            metal_sources: vec![PathBuf::from("shaders/default.metal")],
        }
    }

    fn backend_config(backend: NativeBackend, available: bool) -> NativeBackendConfig {
        NativeBackendConfig {
            backend,
            available,
            unavailable_reason: if available {
                None
            } else {
                Some("not shipped".to_string())
            },
            prebuilt: Some(PathBuf::from(format!("backend/{}.a", backend.as_str()))),
            frameworks: vec![format!("{}Framework", backend.as_str())],
            libs: vec![format!("{}_sys", backend.as_str())],
            lib_dirs: vec![PathBuf::from(format!("vendor/{}/lib", backend.as_str()))],
            pkg_config: vec![format!("{}-pkg", backend.as_str())],
            shader_sources: vec![PathBuf::from(format!("shaders/{}.src", backend.as_str()))],
            shader_outputs: vec![PathBuf::from(format!("shaders/{}.bin", backend.as_str()))],
            resources: vec![PathBuf::from(format!("resources/{}", backend.as_str()))],
            package: NativeBackendPackageMetadata {
                name: Some(format!("demo-{}", backend.as_str())),
                version: Some("1.0.0".to_string()),
                kind: Some("shader-package".to_string()),
            },
        }
    }

    #[test]
    fn selection_includes_available_backend_link_metadata() {
        let mut tc = target_config();
        tc.backends
            .push(backend_config(NativeBackend::Vulkan, true));
        tc.backends
            .push(backend_config(NativeBackend::D3d12, false));

        let selection = select_available_backend_link_metadata(&tc);

        assert_eq!(selection.len(), 1);
        let backend = &selection[0];
        assert_eq!(backend.backend, NativeBackend::Vulkan);
        assert_eq!(backend.prebuilt, Some(PathBuf::from("backend/vulkan.a")));
        assert_eq!(backend.frameworks, vec!["vulkanFramework".to_string()]);
        assert_eq!(backend.libs, vec!["vulkan_sys".to_string()]);
        assert_eq!(backend.lib_dirs, vec![PathBuf::from("vendor/vulkan/lib")]);
        assert_eq!(backend.pkg_config, vec!["vulkan-pkg".to_string()]);
    }

    #[test]
    fn unavailable_target_selects_no_backend_link_metadata() {
        let mut tc = target_config();
        tc.available = false;
        tc.backends
            .push(backend_config(NativeBackend::Vulkan, true));

        let selection = select_available_backend_link_metadata(&tc);

        assert!(selection.is_empty());
    }





}

/// Walk up from the entry `.ts` to the directory holding `perry.toml`.
/// Mirrors `widget_build::project_root_for` — kept local so the link
/// module doesn't reach across sibling modules for one path lookup.
fn find_project_root_for(input: &Path) -> Option<PathBuf> {
    let mut dir = input.canonicalize().ok()?;
    for _ in 0..8 {
        dir = dir.parent()?.to_path_buf();
        if dir.join("perry.toml").exists() {
            return Some(dir);
        }
    }
    None
}

/// Resolve the vendored optional-framework search directory for a native
/// library that gates an SDK on `frameworks_env` (issue #1303 — e.g.
/// `@perryts/google-auth`'s `PERRY_GOOGLE_SIGN_IN_FRAMEWORK_DIR`).
///
/// Precedence (matches the issue's contract):
///   1. The `frameworks_env` env var, when set in the process environment
///      — today's local `perry compile` behavior. Returned verbatim.
///   2. `perry.toml [google_auth].framework_dir`, resolved relative to the
///      project root → absolute. This is what survives the `perry publish`
///      worker round-trip: the dev's shell env doesn't transfer and an
///      absolute local path wouldn't exist on the worker, but perry.toml +
///      the project-relative dir are both uploaded with `--project`, so the
///      worker's `perry compile` re-resolves the same dir.
///
/// Returns `None` when neither source yields a path. Callers still check
/// `is_dir()` before linking, so a stale/misspelled path skips silently
/// (the wrapper's `#if canImport(...)` fallback keeps the link valid).
fn resolve_optional_framework_dir(env_name: &str, args_input: &Path) -> Option<PathBuf> {
    // 1. Explicit env var wins (today's behavior).
    if let Some(dir) = std::env::var_os(env_name) {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    // 2. Project-relative `perry.toml [google_auth].framework_dir`.
    let project_root = find_project_root_for(args_input)?;
    let content = fs::read_to_string(project_root.join("perry.toml")).ok()?;
    let doc = content.parse::<toml::Table>().ok()?;
    let rel = doc
        .get("google_auth")?
        .as_table()?
        .get("framework_dir")?
        .as_str()?;
    Some(project_root.join(rel))
}

/// `PERRY_EXTRA_LINK_ARGS`: whitespace-separated arguments appended verbatim,
/// last, to every final link perry runs: the executable, the dylib, the proven
/// tiny program and the WASI component. A diagnostic input (e.g.
/// `-Wl,-why_live,_js_fs_open` to ask ld64 why a symbol survived dead-strip,
/// or a layout-pad object for startup A/Bs); not part of any documented
/// stable surface. One reader, so no link path silently drops it.
pub(super) fn extra_link_args() -> Vec<String> {
    split_link_env("PERRY_EXTRA_LINK_ARGS")
}

/// `PERRY_LINK_PAD`: whitespace-separated pad objects placed on the link line
/// right after the compiled objects and before the runtime archive (and any
/// other library), on every final link perry runs. A measurement input for
/// startup A/Bs: the linker lays input sections out in command-line order, so
/// a pad here moves every runtime byte of the section it pads, which an
/// input appended last (`PERRY_EXTRA_LINK_ARGS`) cannot do. Not part of any
/// documented stable surface; unset, the link line is unchanged. One reader,
/// so no link path silently drops it.
pub(super) fn link_pad_args() -> Vec<String> {
    split_link_env("PERRY_LINK_PAD")
}

fn split_link_env(name: &str) -> Vec<String> {
    std::env::var(name)
        .map(|value| value.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// Quote one linker argument for a response file. `msvc` selects `link.exe` /
/// `lld-link` rules (a `"` toggles a quoted run; backslashes are literal Windows
/// path separators, so they are NOT escaped — only an embedded `"` is escaped as
/// `\"`); otherwise GNU/clang rules (`\` and `"` escape the next character, both
/// inside and outside double quotes, so both are backslash-escaped). An argument
/// with no whitespace/quote and no backslash needs no quoting in either dialect —
/// the common case (long object/lib paths).
pub(super) fn quote_response_arg(arg: &str) -> String {
    let needs_quote = arg.is_empty()
        || arg
            .bytes()
            .any(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'"'));
    if !needs_quote {
        // #5740 / #6333 — GNU-style tokenizers (clang, gcc, ld: everything
        // reached through a non-MSVC driver) treat `\` as an escape character
        // *outside* quotes too, not just inside them. A plain Windows object
        // path has no spaces, so it took this early return verbatim and the
        // driver ate every separator: `C:\Users\me\AppData\...\app_ts.o` came
        // back out of the tokenizer as `C:UsersmeAppData...app_ts.o` ("it seems
        // the dir replace the \ split"). This bites the Windows-host →
        // Android/HarmonyOS/Linux cross-links, which use the NDK/SDK clang
        // driver (GNU rules) while the response-file path itself is taken on
        // every Windows host.
        //
        // Escape rather than rewrite to `/`: doubling the backslash round-trips
        // the argument byte-for-byte through the tokenizer on *any* host, so a
        // POSIX file whose name legitimately contains a backslash still links
        // (a blanket `\` → `/` substitution would corrupt it).
        if arg.contains('\\') {
            return arg.replace('\\', "\\\\");
        }
        return arg.to_string();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    for c in arg.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Argument bytes above which a link goes through a response file on every
/// host. macOS caps a spawn's argument vector plus environment at 1 MiB
/// (`ARG_MAX`), and a large program's object list passes that: opencode's
/// ~7,000 modules failed with `Argument list too long (os error 7)`. Linux
/// allows more in total, but this keeps every host far from its limit.
pub(super) const INLINE_LINK_ARG_BYTES_MAX: usize = 256 * 1024;

/// Do these link arguments need a response file instead of the command line?
pub(super) fn link_args_need_response_file<'a>(
    args: impl IntoIterator<Item = &'a std::ffi::OsStr>,
) -> bool {
    args.into_iter().map(|a| a.len() + 1).sum::<usize>() > INLINE_LINK_ARG_BYTES_MAX
}

/// Render a linker `Command`'s full argument vector as response-file content
/// (one quoted arg per line). Pure + unit-tested; the `\n` separator and
/// per-arg quoting are what `link.exe`/`lld-link`/`clang` accept via `@file`.
pub(super) fn response_file_contents(args: &[String]) -> String {
    let mut s = String::new();
    for a in args {
        s.push_str(&quote_response_arg(a));
        s.push('\n');
    }
    s
}


/// Render the `@<file>` argument that points the linker at the response file.
///
/// This one is passed on the *command line*, so it is not subject to the
/// response-file tokenizer — but on a Windows host it may still travel through
/// a `cmd.exe`-mediated driver wrapper (the NDK ships `.cmd` shims, and Rust
/// spawns those via `cmd.exe`). Every non-MSVC driver we hand it to (clang,
/// gcc, ld) accepts `/` as a path separator on Windows, so normalizing the
/// separators there sidesteps that whole layer of quoting (#5740).
///
/// `windows_host` gates the rewrite: on POSIX a backslash is a legal filename
/// character, so the path is passed through untouched.
pub(super) fn response_file_arg(rsp_display: &str) -> String {
    format!("@{}", rsp_display)
}

/// Rewrite a linker invocation to pass its arguments via a response file
/// (`<linker> @<file>`) instead of inline, dodging the Windows `CreateProcess`
/// command-line length cap (os error 206) on links with many object files.
/// Preserves program, env overrides, and working directory. The response file
/// goes in the OS temp dir under a per-process/-output unique name (so parallel
/// links don't clobber each other) and the caller deletes it after the link.
/// Returns `None` if there's nothing to gain (no args) or the file can't be
/// written — the caller then keeps the original inline command.
fn rewrite_link_with_response_file(cmd: &Command) -> Option<(Command, PathBuf)> {
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    if args.is_empty() {
        return None;
    }
    let contents = response_file_contents(&args);
    let rsp = std::env::temp_dir().join(format!("perry-link-{}.rsp", std::process::id()));
    fs::write(&rsp, contents).ok()?;

    let mut new_cmd = Command::new(cmd.get_program());
    new_cmd.arg(response_file_arg(
        &rsp.display().to_string(),
    ));
    // Preserve env overrides (e.g. MSVC LIB/PATH set by select_linker_command)
    // and the working directory the original command was configured with.
    for (key, val) in cmd.get_envs() {
        match val {
            Some(v) => {
                new_cmd.env(key, v);
            }
            None => {
                new_cmd.env_remove(key);
            }
        }
    }
    if let Some(cwd) = cmd.get_current_dir() {
        new_cmd.current_dir(cwd);
    }
    Some((new_cmd, rsp))
}

/// `--start-group` / `--end-group` around the perry static-archive block on
/// ELF links (#8930).
///
/// GNU `ld` scans each archive on the command line exactly once, left to
/// right, and pulls only the members that resolve symbols undefined *at that
/// moment*. The perry archive block is mutually recursive, and the direction
/// of that recursion is the opposite of the command-line order:
///
///   * the user objects reference `js_*` symbols that **perry-stdlib**
///     provides, so the well-known wrapper archives listed BEFORE stdlib
///     resolve nothing at all — in the #8930 repro not one of the 92
///     eventually-linked wrapper members was pulled from that first listing;
///   * every wrapper member came from the repeat AFTER stdlib, on references
///     that stdlib's own members had just opened (`js_node_http_*`, …);
///   * those wrapper members then reference back INTO stdlib, which `ld` has
///     already walked past and never revisits.
///
/// While a wrapper archive bundled its own copy of everything it closed over,
/// the third step was invisible. It turns into a hard error the moment
/// `strip_bundled_shared_deps_from_well_known_lib` drops a bundled member
/// *because stdlib provides it* — a correct decision by the archive index,
/// but one that only holds if `ld` can still get back to stdlib. #8930 is
/// exactly that shape: `futures_channel::mpsc::SenderTask::notify`, exported
/// by the linked `libperry_stdlib.a`'s own `futures_channel` member and
/// referenced by a kept `perry_ext_http` codegen unit, with no path from the
/// one to the other.
///
/// A group makes `ld` re-scan the block until it reaches a fixed point, which
/// is the guarantee a mutually recursive archive set needs. Repeating a single
/// archive (the codebase's usual "archive twice" trick — see the GTK4 stdlib
/// re-link) only covers a one-step cycle, and this graph is LTO-partitioned
/// across hundreds of codegen units on both sides. Symbol precedence is
/// unchanged: members are still pulled left to right, so the
/// wrappers-before-stdlib and localized-runtime-last first-definition-wins
/// ordering keeps working.
///
/// ELF only. Mach-O's `ld64` already resolves archives to a fixed point (and
/// rejects the flag); `lld-link` / MSVC `link.exe` have no group concept.
pub(super) struct ElfArchiveGroup {
    opened: bool,
}

impl ElfArchiveGroup {
    /// Emit `-Wl,--start-group` when this is an ELF link that will actually
    /// receive archives. `has_archives` keeps an empty group off the command
    /// line for the link shapes that add none.
    pub(super) fn open(cmd: &mut Command, is_elf: bool, has_archives: bool) -> Self {
        let opened = is_elf && has_archives;
        if opened {
            cmd.arg("-Wl,--start-group");
        }
        Self { opened }
    }

    /// Close a group opened by [`ElfArchiveGroup::open`]. Takes `self` by
    /// value so the marker cannot be emitted twice.
    pub(super) fn close(self, cmd: &mut Command) {
        if self.opened {
            cmd.arg("-Wl,--end-group");
        }
    }
}

#[cfg(test)]
mod elf_archive_group_tests {
    use super::ElfArchiveGroup;
    use std::process::Command;

    fn args(is_elf: bool, archives: &[&str]) -> Vec<String> {
        let mut cmd = Command::new("cc");
        let group = ElfArchiveGroup::open(&mut cmd, is_elf, !archives.is_empty());
        for archive in archives {
            cmd.arg(archive);
        }
        group.close(&mut cmd);
        cmd.get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    /// #8930 — an ELF link brackets the archive block so `ld` re-scans it to a
    /// fixed point instead of dying on a stdlib symbol it already walked past.
    #[test]
    fn elf_link_brackets_the_archive_block() {
        assert_eq!(
            args(true, &["wrapper.a", "libperry_stdlib.a", "wrapper.a"]),
            vec![
                "-Wl,--start-group",
                "wrapper.a",
                "libperry_stdlib.a",
                "wrapper.a",
                "-Wl,--end-group",
            ]
        );
    }

    /// Mach-O's `ld64` rejects the flag and needs no group; `lld-link` / MSVC
    /// have no group concept. Those command lines must come out unchanged.
    #[test]
    fn non_elf_link_is_untouched() {
        assert_eq!(
            args(false, &["libperry_stdlib.a"]),
            vec!["libperry_stdlib.a"]
        );
    }

    /// A link shape that contributes no archive must not emit an empty group.
    #[test]
    fn empty_archive_block_emits_no_group() {
        assert!(args(true, &[]).is_empty());
    }
}

#[cfg(test)]
mod link_input_env_tests {
    use super::split_link_env;

    /// The pad and extra-argument inputs share one reader: whitespace-separated, empty when unset.
    /// (A name no perry code reads, so parallel tests cannot see it.)
    #[test]
    fn link_inputs_split_on_whitespace_and_default_to_empty() {
        const NAME: &str = "PERRY_TEST_LINK_INPUT_SPLIT_ONLY";
        std::env::remove_var(NAME);
        assert!(split_link_env(NAME).is_empty());
        std::env::set_var(NAME, "  a.o\t-Wl,-x  b.o\n");
        assert_eq!(split_link_env(NAME), ["a.o", "-Wl,-x", "b.o"]);
        std::env::remove_var(NAME);
    }
}

#[cfg(test)]
mod optional_framework_dir_tests;

#[cfg(test)]
mod response_file_tests {
    use super::{
        link_args_need_response_file, quote_response_arg, response_file_arg,
        response_file_contents, INLINE_LINK_ARG_BYTES_MAX,
    };

    #[test]
    fn large_link_argument_lists_take_a_response_file() {
        use std::ffi::OsStr;
        let small = ["-o", "out", "a.o"].map(OsStr::new);
        assert!(!link_args_need_response_file(small));
        let object = "/private/tmp/perry/very/long/object/path/module_000000.o";
        let count = INLINE_LINK_ARG_BYTES_MAX / (object.len() + 1) + 1;
        let many = vec![OsStr::new(object); count];
        assert!(link_args_need_response_file(many));
        let fits = vec![OsStr::new(object); count - 1];
        assert!(!link_args_need_response_file(fits));
    }

    #[test]
    fn plain_paths_are_unquoted_in_both_dialects() {
        assert_eq!(quote_response_arg("/tmp/a_ts.o"), "/tmp/a_ts.o");
        assert_eq!(
            quote_response_arg(r"C:\build\d0449_ts.o"),
            r"C:\build\d0449_ts.o"
        );
        // /OPT:REF etc. — no whitespace, untouched.
        assert_eq!(quote_response_arg("/OPT:REF"), "/OPT:REF");
    }

    /// #5740 / #6333 — the regression this PR fixes. A Windows object path has
    /// no spaces, so it used to take the "no quoting needed" early return
    /// verbatim; the GNU tokenizer in clang/gcc/ld then consumed each `\` as an
    /// escape and the path collapsed to `C:Usersme...`. Every separator must
    /// come out the other side, so each one is doubled.
    #[test]
    fn gnu_escapes_backslashes_in_unquoted_windows_paths() {
        assert_eq!(
            quote_response_arg(r"C:\Users\me\AppData\Local\Temp\app_ts.o"),
            r"C:\\Users\\me\\AppData\\Local\\Temp\\app_ts.o"
        );
        // Same for flags that embed a path (`-Wl,-rpath,C:\lib`).
        assert_eq!(
            quote_response_arg(r"-Wl,--out-implib,C:\build\app.lib"),
            r"-Wl,--out-implib,C:\\build\\app.lib"
        );
        // MSVC keeps backslashes literal — link.exe/lld-link do not escape.
        assert_eq!(
            quote_response_arg(r"C:\Users\me\app_ts.o"),
            r"C:\Users\me\app_ts.o"
        );
        // Unix paths have no backslash to escape: byte-identical passthrough.
        assert_eq!(
            quote_response_arg("/tmp/perry/app_ts.o"),
            "/tmp/perry/app_ts.o"
        );
    }

    /// The `@<file>` arg rides the command line, not the response file. On a
    /// Windows host with a non-MSVC driver it is normalized to `/` separators
    /// (clang/gcc accept them, and it survives any `cmd.exe` wrapper); on POSIX
    /// a backslash is a legal filename character and must survive untouched.
    #[test]
    fn response_file_arg_normalizes_only_on_windows_host_gnu_driver() {
        assert_eq!(
            response_file_arg(r"C:\Temp\perry-link-1.rsp"),
            "@C:/Temp/perry-link-1.rsp"
        );
        // MSVC driver on Windows: backslashes are the native separator.
        assert_eq!(
            response_file_arg(r"C:\Temp\perry-link-1.rsp"),
            r"@C:\Temp\perry-link-1.rsp"
        );
        // POSIX host: never rewrite (a `\` here is part of the filename).
        assert_eq!(
            response_file_arg(r"/tmp/we\ird/perry-link-1.rsp"),
            r"@/tmp/we\ird/perry-link-1.rsp"
        );
        assert_eq!(
            response_file_arg("/tmp/perry-link-1.rsp"),
            "@/tmp/perry-link-1.rsp"
        );
    }

    #[test]
    fn msvc_quotes_spaces_keeps_backslashes_literal() {
        // Windows path with a space: quoted, backslashes NOT escaped.
        assert_eq!(
            quote_response_arg(r"C:\Program Files\x.lib"),
            "\"C:\\Program Files\\x.lib\""
        );
    }

    #[test]
    fn gnu_quotes_and_escapes_backslashes() {
        assert_eq!(quote_response_arg("/a b/x.o"), "\"/a b/x.o\"");
        assert_eq!(
            quote_response_arg(r"/a\b c/x.o"),
            "\"/a\\\\b c/x.o\""
        );
    }

    #[test]
    fn embedded_quote_is_escaped() {
        assert_eq!(quote_response_arg("a\"b"), "\"a\\\"b\"");
        assert_eq!(quote_response_arg("a\"b"), "\"a\\\"b\"");
    }


    #[test]
    fn contents_is_one_arg_per_line() {
        let args = vec![
            "/tmp/main.o".to_string(),
            "/tmp/mod 1.o".to_string(),
            "-lperry".to_string(),
        ];
        assert_eq!(
            response_file_contents(&args),
            "/tmp/main.o\n\"/tmp/mod 1.o\"\n-lperry\n"
        );
    }
}

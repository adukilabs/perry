# Perry, adukilabs fork

A cut-down fork of [Perry](https://github.com/PerryTS/perry), a compiler that turns TypeScript
into native machine code ahead of time. This fork keeps one thing: compiling TypeScript for
Linux, as an executable or as a static library that a host program links.

It exists for an application platform that runs HTML, CSS and TypeScript apps directly on the
Linux kernel. That platform needs a TypeScript compiler it can read, build and change. It does
not need Perry's support for other operating systems, its UI toolkits or its WebAssembly target.

Perry is the work of the Perry contributors and is MIT licensed. The license and copyright
notice are in `LICENSE`, unchanged. For Perry itself, its documentation and its releases, go to
the upstream repository. Nothing here is endorsed by the Perry project.

## Branches

| Branch | What it is |
|---|---|
| `main` | The fork. Upstream `main`, cut down, on LLVM 23, with four fixes. Build this. |
| `upstream` | Upstream `main`, untouched. For reference and for comparing against. It is moved forward to follow Perry and nothing of ours is committed to it. |
| `platform-fixes` | Upstream release `v0.5.1665` plus three of the fixes, nothing removed, LLVM 22. The fixes were measured here. |
| `platform` | An earlier cut, based on release `v0.5.1665`. Superseded by `main`. |

Other branch names in this repository came with the fork and are upstream's.

## What is different from upstream

**Four fixes for static library output.**

| Fault | Fix | Upstream report |
|---|---|---|
| A plain link (`cc host.c libapp.a ...`) left out the object that sets up big integers, regular expressions and more. A regex replace then returned its input with exit code 0. | The library's entry function calls the set-up function, so any link keeps it. | PerryTS/perry issue 14787 |
| `worker.terminate()` on a worker that only loops aborted the process. | Every function of a module that receives loop polls gets an unwind table entry. | PerryTS/perry issue 14788 |
| The exception object was aligned to 8 bytes where the ABI asks for 16. LLVM's unwinder faults on that. | `align(16)` on the object. | PerryTS/perry issue 14789 |
| On upstream `main`, every exported function of a static library compiled to a single trap instruction, and the host's first call died with an illegal instruction. Dead-code removal treated functions that only the host calls as unreachable. | Library output keeps every function body. | not reported |

The fourth fault is not in release `v0.5.1665`. It arrived on upstream `main` afterwards.

**One more difference in behaviour.** A native library package declares its files per system
in `perry.nativeLibrary.targets`. Upstream reads the `macos` section for the target names
`linux-x86_64`, `linux-arm64` and `linux-aarch64`, because its lookup falls through to
`macos` for any name it does not list (read from the source, not run). This fork reads the
`linux` section for every target it accepts, and the per-architecture key (`linux-x64`,
`linux-arm64`) follows the target name. Checked by unit tests only.

**LLVM 23 in place of LLVM 22.** LLVM 23 split the branch instruction code in two, which five
comparisons in the code generator now follow. It also removed the `Os` and `Oz` pass pipelines.
Both size levels now run the `O2` pipeline and rely on the `optsize` and `minsize` attributes
Perry already puts on app code.

**Removed.**

- Half of the workspace packages: the UI toolkits for macOS, iOS, Android, Windows, GTK and the
  rest, six code generators for other platforms, optional extension packages (SQLite, PDF,
  image processing and others), and WebAssembly hosting.
- The WebAssembly target of the code generator, and every script, test and fixture for it.
- Every module compiled only for Windows, an Apple system or WASI, and the scripts, tests and
  documentation pages named for another operating system.
- Inside the kept files, the items, fields, match arms and statements under a `cfg` condition
  that cannot hold on x86-64 Linux, and the `if cfg!(...)` branches decided there. Six files
  that do not parse on their own were left as they are.
- Every `--target` other than Linux. `compile`, `check`, `lock`, `run` and `target add` refuse
  any other name with a message that this fork is for Linux only, before anything is written.
  The accepted names are `linux`, `linux-x86_64`, `linux-arm64`, `linux-aarch64`, `linux-musl`,
  `linux-x86_64-musl` and `linux-aarch64-musl`.
- In the compiler package, the code that only those other targets reached: the link steps for
  Android, Apple systems, Windows and HarmonyOS, the macOS app bundle, the Windows and Android
  target tables, and what was left unused once they were gone. That package went from 332
  Rust files and 144,336 lines to 294 files and 123,008 lines. Upstream has 344 and 151,895.
- Benchmarks, changelogs, CI workflows, packaging, vendored CI actions, and translated docs.
- The publish and mobile commands are not offered. The built compiler offers `compile`,
  `check`, `init`, `install`, `doctor`, `explain`, `update`, `run`, `dev`, `i18n`, `login`,
  `types`, `cache`, `target` and `lock` (read from its `--help`). About 1,900 lines of the
  publish code remain, because `login`, `update` and the remote build of `run` use its
  configuration file and its upload code.

| | Upstream `main` | This fork's `main` |
|---|---|---|
| Tracked files | 44,326 | 18,064 |
| Locked packages | 973 | 656 |
| Size without `.git` | about 1.4 GB | 176 MB |

**Not removed yet.**

- 160 `cfg` attributes in 78 files that still name another system, against 2,892 in 406
  upstream, by a text search. They sit inside macro definitions, in the six files that do
  not parse on their own, or are one part of a wider condition.
- The removal of other targets was done in the compiler package only. The runtime, the code
  generator and the other packages were not gone over for code that only another target
  reaches.
- In the compiler package, 1,161 lines in 110 files still contain the name of another system
  or of its tools, by a word search (4,278 lines in 155 files before). Sorted by a script:
  540 are comment lines, 152 are in test files, and 468 are code or message text. Of those
  468, read from a random sample of 40: options that are still accepted and have no effect
  (`--min-windows-version` and `--windows-subsystem` are checked and stored and nothing reads
  what is stored, the HarmonyOS signing options are not read at all), the import of `.wasm`
  files as assets, and 8 places in 3 files that branch on the text of a target triple
  (`contains("windows")`, `contains("apple-")`, `starts_with("wasm32")`). Every triple comes
  from a Linux target name or is the one this compiler was built for, so those branches are
  not taken (read from the callers). The tool that removed the rest does not see them.
- The `update` and `login` commands and the remote build of `run` talk to Perry's own
  servers. They are built and were not tried.
- `tests/parity`, 9,070 of upstream's 9,151 files of behaviour tests.

## Building

Needs: the Rust toolchain named in `rust-toolchain.toml`, LLVM 23 development files, `clang`,
`cmake`, and a C compiler.

```bash
# LLVM_SYS_231_PREFIX is needed only when llvm-config is not on PATH.
export LLVM_SYS_231_PREFIX=/usr/lib/llvm-23
export PERRY_TRAP_CLANG=clang-23        # only when plain `clang` is not on PATH

# One cargo run for all four. Built in separate runs, the runtime and standard
# library archives hold different copies of the runtime and do not link together.
cargo build -p perry -p perry-runtime-static -p perry-stdlib-static -p perry-gc-effects
```

That is a debug build, the one to work with. Measured on a 12-thread laptop with four jobs and
the build held to 5 GB of memory: about 10 minutes from nothing, and 2 minutes after a change.

For optimized output add `--profile prod` to the same command, then write the sidecar file the
compiler looks for beside the runtime archive:

```bash
target/prod/perry-gc-effects sidecars target/prod
```

Without the sidecar the compiler prints a warning and treats every runtime call as one that can
throw, which is correct and slower. On the debug archives the sidecar tool took 8 to 10 minutes
here.

A compiler run from this checkout's own `target` directory refuses to compile until the sidecar
is there. It tells its workspace by `crates/perry-runtime` and `crates/perry-stdlib`.

```bash
target/debug/perry compile app.ts -o app                              # an executable
target/debug/perry compile app.ts --output-type staticlib -o libapp.a # a library for a host
```

## What has been checked, and what has not

Checked on `main`, with a debug build on Debian with LLVM 23.1.6:

- It builds with no errors.
- Unit tests. The compiler's were run at `7afc456e`, the code generator's at `ea75af9f` and
  the others at `1405f4b8`. No file of a package changed between its run and `7afc456e`.
  Where a suite failed it was also run on unmodified upstream `main` at 7270b624, with
  LLVM 22, and the failures compared by name.

  | Package | This fork | Upstream `main` |
  |---|---|---|
  | `perry-runtime` | 6,870 passed, 0 failed | not run |
  | `perry-codegen` | 2,975 passed, 6 failed | 2,974 passed, the same 6 failed |
  | `perry` (the compiler) | 1,437 passed, 0 failed | 1,627 passed, 0 failed |
  | `perry-hir`, `perry-transform`, `perry-parser`, `perry-gc-effects` | 893, 395, 50 and 43 passed, 0 failed | not run |

  The compiler has 190 fewer tests than upstream. Tests of removed code went with it, and 15
  tests that asserted only how another system is handled were cut. 25 tests reached kept
  code through a macOS, iOS, Windows or WebAssembly fixture or target name: 24 were rewritten
  on Linux targets and ELF fixtures, and one passed once stale entries were taken out of the
  list it checks. Two guards on how much a scan saw were lowered to fit this fork:
  at least 20 packages, and at least 40 manifests where the fork has 41.
- The gate on `--target`, with the built compiler: no target, `linux` and `linux-x86_64`
  each compile a program that runs and prints the expected line. 15 other names, among them
  `macos`, `ios`, `windows`, `android`, `wasi` and one misspelt Linux name, are each refused
  with exit status 1 and write no output. `check`, `lock` and four `target add` requests for
  another system are refused the same way. 24 of 24 cases.
- `--output-type dylib` writes an ELF shared object, and `perry run` compiles and runs a
  program. One run each.
- A static library linked plainly behaves the same as one linked with every member forced in,
  for six runtime features, and all twelve runs exit 0.
- `worker.terminate()` stops a looping worker in executable and static library output, in 15
  of 15 runs. Whether this run exercises the unwind table fix was not determined: earlier
  runs had no sidecar and did not. That fix was checked on `platform-fixes` with an optimized
  build on LLVM 22, where the static library case went from an abort in 3 of 3 runs to
  stopping in 6 of 6.
- The two checks above were last run at `7afc456e`, with a sidecar beside the debug archives.

Not checked:

- The unit tests of `perry-stdlib`, the integration tests under `crates/*/tests`, and the
  behaviour tests in `tests/parity`.
- An optimized build on LLVM 23, and code size or speed against LLVM 22.
- The fix for the exception object's alignment has been read from the built archive (the
  object's section asks for 16) but the fault itself has not been re-run against it.
- Any target other than x86-64 Linux with glibc. `linux-arm64`, `linux-aarch64` and the three
  musl names are accepted and were not compiled: this machine has no cross or musl toolchain.

## Keeping up with upstream

`upstream` follows Perry's `main`. To bring `main` forward, merge `upstream` into a copy of
`main` and run the cut script again. The script works from the dependency graph and from names,
so it finds what to remove without a fixed list. Expect to fix by hand whatever upstream added
that names LLVM 22, and to check static library output, which upstream does not test on Linux.

## License

MIT. See `LICENSE`.

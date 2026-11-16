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
- Benchmarks, changelogs, CI workflows, packaging, vendored CI actions, and translated docs.
- The compiler's default command set is the slim one (`compile`, `run`, `check`, `types`,
  `cache`, `dev`). The publish, mobile and updater commands are still in the source and are
  built only with `--features full-cli`.

| | Upstream `main` | This fork's `main` |
|---|---|---|
| Tracked files | 44,326 | 18,118 |
| Locked packages | 973 | 656 |
| Size without `.git` | about 1.4 GB | 177 MB |

**Not removed yet.**

- About 990 functions, blocks and statements inside kept files that are marked for another
  operating system or for WebAssembly. None is compiled on Linux.
- The compiler's own link and run steps for Android, Apple systems, Windows and HarmonyOS.
  Those are chosen at run time by `--target`, so they are ordinary code in a Linux build.
- `tests/parity`, 9,150 files of behaviour tests, kept whole.

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
throw, which is correct and slower. The sidecar tool refused the debug archives here.

```bash
target/debug/perry compile app.ts -o app                              # an executable
target/debug/perry compile app.ts --output-type staticlib -o libapp.a # a library for a host
```

## What has been checked, and what has not

Checked on `main`, with a debug build on Debian with LLVM 23.1.6:

- It builds with no errors, and the unit tests of the compiler, the code generator and the
  runtime compile.
- A static library linked plainly behaves the same as one linked with every member forced in,
  for six runtime features, and all twelve runs exit 0.
- `worker.terminate()` stops a looping worker in executable and static library output, in 15
  of 15 runs. A debug build has no sidecar, so this run does not exercise the unwind table
  fix. That fix was checked on `platform-fixes` with an optimized build on LLVM 22, where the
  static library case went from an abort in 3 of 3 runs to stopping in 6 of 6.

Not checked:

- Perry's own test suite has not been run on this branch, apart from 44 unit tests of the code
  generator passes this fork changed (loop polls, stack checks, cold paths, native homes),
  which pass.
- An optimized build on LLVM 23, and code size or speed against LLVM 22.
- The fix for the exception object's alignment has been read from the built archive (the
  object's section asks for 16) but the fault itself has not been re-run against it.
- Any target other than x86-64 Linux.

## Keeping up with upstream

`upstream` follows Perry's `main`. To bring `main` forward, merge `upstream` into a copy of
`main` and run the cut script again. The script works from the dependency graph and from names,
so it finds what to remove without a fixed list. Expect to fix by hand whatever upstream added
that names LLVM 22, and to check static library output, which upstream does not test on Linux.

## License

MIT. See `LICENSE`.

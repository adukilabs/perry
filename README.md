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
| `main` | Upstream `main` as it was when the fork was made. Not built on. |
| `platform-fixes` | Upstream release `v0.5.1665` plus three fixes, nothing removed. |
| `platform` | The branch to use. `platform-fixes`, cut down, on LLVM 23. |

## What is different from upstream

**Three fixes for static library output.** Each was reported upstream with a reproduction.

| Fault | Fix | Upstream report |
|---|---|---|
| A plain link (`cc host.c libapp.a ...`) left out the object that sets up big integers, regular expressions and more. A regex replace then returned its input with exit code 0. | The library's entry function calls the set-up function, so any link keeps it. | PerryTS/perry issue 14787 |
| `worker.terminate()` on a worker that only loops aborted the process. | Every function of a module that receives loop polls gets an unwind table entry. | PerryTS/perry issue 14788 |
| The exception object was aligned to 8 bytes where the ABI asks for 16. LLVM's unwinder faults on that. | `align(16)` on the object. | PerryTS/perry issue 14789 |

**LLVM 23 in place of LLVM 22.** Two changes were needed. LLVM 23 split the branch instruction
code in two, and it removed the `Os` and `Oz` pass pipelines. Both size levels now run the `O2`
pipeline and rely on the `optsize` and `minsize` attributes Perry already puts on app code.

**Removed.**

- 37 of 72 workspace packages: the UI toolkits for macOS, iOS, Android, Windows, GTK and the
  rest, six code generators for other platforms, optional extension packages (SQLite, PDF,
  image processing and others), and WebAssembly hosting.
- The WebAssembly target of the code generator.
- Benchmarks, changelogs, CI workflows, packaging, vendored CI actions, and translated docs.
- The compiler's default command set is the slim one (`compile`, `run`, `check`, `types`,
  `cache`, `dev`). The publish, mobile and updater commands are still in the source and are
  built only with `--features full-cli`.

The tree is 159 MB with 16,208 files. Upstream at the same release is 1.4 GB with 41,793.

**One upstream change taken since the fork.** `fix(strip-dedup)` (upstream #13554), a link fix.
It builds here and its own unit test passes.

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
the build held to 5 GB of memory: about 10 minutes from nothing (before the cut), and 1 to 4
minutes after a change.

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

Checked on `platform`, with a debug build on Debian with LLVM 23.1.6:

- It builds with no errors.
- A static library linked plainly behaves the same as one linked with every member forced in,
  for six runtime features.
- `worker.terminate()` stops a looping worker in executable and static library output, in 15
  of 15 runs. A debug build has no sidecar, so this run does not exercise the unwind table
  fix. That fix was checked on `platform-fixes` with an optimized build on LLVM 22, where the
  static library case went from an abort in 3 of 3 runs to stopping in 6 of 6.

Not checked:

- Perry's own test suite has not been run on this branch.
- An optimized build on LLVM 23, and code size or speed against LLVM 22.
- The fix for the exception object's alignment has been read from the built archive (the
  object's section asks for 16) but the fault itself has not been re-run against it.
- Any target other than x86-64 Linux. The source for other targets that remains inside the
  kept packages is untested and is expected to go.

## Keeping up with upstream

`platform` is based on the release `v0.5.1665`, which is 1,459 commits behind upstream `main`,
so upstream commits often do not apply as they are. Of the 12 commits upstream made in the
hours after the fork, one was taken. The cut itself is made by a script, so it can be repeated
on a newer upstream release.

## License

MIT. See `LICENSE`.

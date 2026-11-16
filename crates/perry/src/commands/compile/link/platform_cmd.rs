//! Per-platform linker-command construction.
//!
//! Split out of `link/mod.rs` so the orchestrator function stays focused on
//! the link-line assembly (objects, libs, frameworks, native libs). This
//! module owns the `if/else if` chain that picks the right toolchain
//! (clang / swiftc / lld / link.exe / ld64.lld) and primes it with the
//! cross-compile flags, sysroots, and entry-symbol rewrites that each
//! platform needs before any of the per-link-line code runs.

use super::glibc_version::glibc_version_defsym;
use super::*;






/// Construct the platform-specific linker `Command` and prime it with the
/// toolchain/sysroot/triple flags that every per-platform branch needs
/// before the orchestrator appends object files and libraries.
///
/// Returns an error for unsupported cross-compile combinations (e.g.
/// visionOS from a non-macOS host).
pub fn select_linker_command(
    target: Option<&str>,
) -> Result<Command> {
    // For cross-compilation targets, use the appropriate toolchain
    let cmd = if matches!(target, Some("linux-arm64") | Some("linux-aarch64")) {
        // aarch64 Linux is a cross-compile (the builder host is x86_64), so the
        // plain host `cc` won't do. Prefer the `aarch64-linux-gnu-gcc` cross
        // toolchain (ships its own sysroot + linker); fall back to clang/cc with
        // an explicit target triple + an optional sysroot from
        // PERRY_LINUX_ARM64_SYSROOT. Mirrors the iOS/Windows cross pattern.
        let cross_gcc = std::env::var("PERRY_LINUX_ARM64_CC")
            .unwrap_or_else(|_| "aarch64-linux-gnu-gcc".to_string());
        let cross_on_path = std::env::var_os("PATH")
            .map(|paths| std::env::split_paths(&paths).any(|d| d.join(&cross_gcc).is_file()))
            .unwrap_or(false);
        let mut c = if cross_on_path {
            Command::new(cross_gcc)
        } else {
            let mut c = Command::new("cc");
            c.arg("-target").arg("aarch64-unknown-linux-gnu");
            c.arg("-fuse-ld=lld");
            if let Ok(sysroot) = std::env::var("PERRY_LINUX_ARM64_SYSROOT") {
                c.arg(format!("--sysroot={}", sysroot));
                c.arg("-Wl,-dynamic-linker=/lib/ld-linux-aarch64.so.1");
            }
            c
        };
        // The program links against the cross sysroot's glibc.
        let defsym = glibc_version_defsym(&c, false)?;
        c.arg(defsym);
        c
    } else if matches!(
        target,
        Some("linux-musl") | Some("linux-x86_64-musl") | Some("linux-aarch64-musl")
    ) {
        // Fully-static musl Linux target (#4826). Produces a binary with no
        // dynamic-loader / glibc dependency, so it runs unchanged on AWS
        // Lambda `provided.al2023` (glibc 2.34), scratch/distroless
        // containers, Cloud Run, etc. — the GLIBC_2.39-not-found failures
        // happen in the *loader*, before Perry's tiny-init ever runs, so a
        // static binary sidesteps them entirely.
        //
        // Driver: prefer the musl gcc wrapper (`musl-gcc` for x86_64, from
        // the `musl-tools` package) which carries musl's specs + sysroot +
        // crt objects. Fall back to clang/cc with an explicit musl triple +
        // a sysroot from PERRY_LINUX_MUSL_SYSROOT (mirrors the glibc-cross
        // fallback below). The perry-runtime / perry-stdlib `.a` files for
        // the musl triple are built by release-packages.yml and resolved by
        // library_search.rs via rust_target_triple().
        let is_musl_aarch64 = target == Some("linux-aarch64-musl");
        let (cc_env, default_musl_gcc, clang_triple) = if is_musl_aarch64 {
            (
                "PERRY_LINUX_MUSL_AARCH64_CC",
                "aarch64-linux-musl-gcc",
                "aarch64-unknown-linux-musl",
            )
        } else {
            (
                "PERRY_LINUX_MUSL_CC",
                "musl-gcc",
                "x86_64-unknown-linux-musl",
            )
        };
        let musl_gcc = std::env::var(cc_env).unwrap_or_else(|_| default_musl_gcc.to_string());
        let musl_gcc_on_path = std::env::var_os("PATH")
            .map(|paths| std::env::split_paths(&paths).any(|d| d.join(&musl_gcc).is_file()))
            .unwrap_or(false);
        let mut c = if musl_gcc_on_path {
            Command::new(musl_gcc)
        } else {
            let mut c = Command::new("cc");
            c.arg("-target").arg(clang_triple);
            c.arg("-fuse-ld=lld");
            if let Ok(sysroot) = std::env::var("PERRY_LINUX_MUSL_SYSROOT") {
                c.arg(format!("--sysroot={}", sysroot));
            }
            c
        };
        // Fully static link: no interpreter, libc/libm/libpthread/libdl all
        // folded into the executable. musl's libc.a is self-contained, so
        // this is the supported/portable mode (unlike a fully-static glibc).
        c.arg("-static");
        c
    } else {
        // Linux target: when running on Linux natively, just use "cc".
        // When cross-compiling from macOS, use clang + ld.lld + a glibc
        // sysroot pointed to by PERRY_LINUX_SYSROOT (matching the
        // PERRY_IOS_SYSROOT/PERRY_WINDOWS_SYSROOT builder pattern).
        // Only the cross-compile block below mutates `c`; building on Linux
        // hands back a bare `cc`.
        let mut c = Command::new("cc");
        // Unresolved symbols are now link errors (not warnings). The
        // v0.5.0→0.5.18 Fastify/MySQL segfault (#28) was caused by
        // --warn-unresolved-symbols silently producing binaries with
        // null function pointers that crashed at runtime. With the
        // native module dispatch table restored, all expected symbols
        // are resolved; any remaining unresolved symbol is a real bug
        // that should fail the link rather than produce a broken binary.
        //
        // Record the glibc the program links against (this host's when
        // linking natively, the sysroot's when cross-linking) for
        // process.report's header.glibcVersionCompiler.
        let defsym = glibc_version_defsym(&c, true)?;
        c.arg(defsym);
        c
    };

    Ok(cmd)
}

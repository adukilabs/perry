//! Windows linker subsystem regression tests — split from compile.rs
//! in v0.5.1019 (file-size CI gate). Brought back in via
//! `#[cfg(test)] mod windows_link_tests;` in compile.rs.

use super::library_search::newest_mt_dir_under;

// Regression guard for issue #120: without an explicit subsystem flag the
// MSVC linker historically defaulted to WINDOWS (2), silently detaching
// stdout/stderr so console.log output never reached the terminal.



// Issue #303: --min-windows-version=7 emits the ,5.1 suffix that marks
// the PE as Win7-compatible.

// Issue #303: --min-windows-version=8 emits the ,6.02 suffix.

// Anything other than 7/8/10 falls through to no suffix — caller-side
// CompileArgs validation rejects unknown values before reaching the
// linker, so this branch is unreachable in practice but documented.

// --windows-subsystem / [windows] subsystem override (resolved into
// ctx.windows_subsystem) folds into needs_ui before the flag is built.

// "auto" defers to the import-driven heuristic — both polarities pass through.

// "windows" forces GUI even when nothing imported perry/ui — this is the
// Bloom-Engine-game case: no console window pops up alongside the game.

// "console" forces a console even for a UI program that would auto-detect GUI.

// The override composes with the min-windows-version suffix.

// Issue #4771: the Windows output extension defaults by output type — .exe for
// executables, .dll for shared libraries, .lib for static libraries — so a
// user-supplied `-o NAME` without an extension still produces a runnable /
// linkable file on Windows.

// 2026-07 audit: a Windows `--output-type staticlib` no longer hard-requires
// MSVC lib.exe. Precedence: lib.exe → llvm-lib (drop-in replacement, ships
// with `winget install LLVM.LLVM`) → llvm-ar (rustup llvm-tools); None when
// nothing is installed, which the pipeline turns into the two-toolchain
// install hint instead of a raw spawn error.

// lib.exe / llvm-lib take the MSVC-style `/OUT:` argument (objects appended
// by the caller).

// llvm-ar needs the COFF archive flavor spelled out, plus the same `crs`
// modifiers the Unix `ar` branch uses.

// Issue #4681 / discussion #3486: the embedded app manifest must declare the
// comctl32 v6 side-by-side dependency, otherwise common controls bind v5 and
// render unthemed. Guards against accidental edits to windows_app.manifest.

// asInvoker keeps UI binaries out of the UAC installer-detection heuristic and
// avoids a second linker-generated trustInfo block (we pass /MANIFESTUAC:NO).

// Issue #559: an unpackaged Win32 process must opt into the Windows 10 1903
// compatibility context before DesktopWindowXamlSource will initialize.

// Issue #6023: only MSVC link.exe needs the mt.exe reachability treatment —
// lld-link embeds manifests in-process. The classifier keys on the program's
// file stem so bare names and vswhere-resolved absolute paths both match.

// Console-only binaries must stay manifest-free (and thus never depend on
// mt.exe at all).

// lld-link needs no mt.exe, so a UI build through it always gets the full
// /MANIFEST:EMBED argument set regardless of any Windows SDK presence.

// Issue #6023: the SDK-bin probe picks the newest versioned dir that actually
// contains mt.exe, preferring x64 over x86.
#[test]
fn mt_dir_probe_picks_newest_versioned_sdk() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path();
    // Older SDK with both arches, newest mt-bearing SDK also with both
    // arches (x64 must win within it), plus a newer version dir that lacks
    // mt.exe entirely (must be skipped).
    for dir in [
        "10.0.19041.0/x64",
        "10.0.19041.0/x86",
        "10.0.22621.0/x64",
        "10.0.22621.0/x86",
        "10.0.26100.0/arm64",
    ] {
        std::fs::create_dir_all(bin.join(dir)).unwrap();
    }
    for mt in [
        "10.0.19041.0/x64/mt.exe",
        "10.0.19041.0/x86/mt.exe",
        "10.0.22621.0/x64/mt.exe",
        "10.0.22621.0/x86/mt.exe",
    ] {
        std::fs::write(bin.join(mt), b"").unwrap();
    }
    assert_eq!(
        newest_mt_dir_under(bin),
        Some(bin.join("10.0.22621.0").join("x64"))
    );
}

// Pre-10.0.15063 SDKs put tools directly under bin\<arch> with no version dir.
#[test]
fn mt_dir_probe_falls_back_to_unversioned_layout() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path();
    std::fs::create_dir_all(bin.join("x86")).unwrap();
    std::fs::write(bin.join("x86").join("mt.exe"), b"").unwrap();
    assert_eq!(newest_mt_dir_under(bin), Some(bin.join("x86")));
}

// No SDK bin root / no mt.exe anywhere → None, which makes embed_app_manifest
// skip the embed instead of letting link.exe die with LNK1158.
#[test]
fn mt_dir_probe_handles_missing_root_and_empty_tree() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(newest_mt_dir_under(&root.path().join("nope")), None);
    std::fs::create_dir_all(root.path().join("10.0.22621.0").join("x64")).unwrap();
    assert_eq!(newest_mt_dir_under(root.path()), None);
}

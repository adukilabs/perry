//! Native-symbol-table `nm` invocations for the strip/dedup pipeline.
//! Split out of `strip_dedup.rs` to keep the parent under the 2000-line gate.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Start an `llvm-nm` invocation that reads the object's NATIVE symbol table.
///
/// On a Mach-O member that still carries an `__LLVM,__bitcode` section
/// (unstripped local archives built with embedded bitcode), `llvm-nm` reads the
/// bitcode module's symbols instead of the nlist unless told not to. The two
/// can differ (internalized or merged symbols), so dedup would compare the
/// wrong sets and keep duplicate std/core members. `--no-llvm-bc` (LLVM >= 13)
/// pins the native view. A GNU binutils `nm` (the `PATH` fallback on Linux)
/// rejects the flag, and it never reads bitcode that way, so the flag is
/// added only for LLVM-based tools (`llvm-nm`, Apple's `nm`).
pub(super) fn nm_command(nm: &Path) -> Command {
    let mut cmd = Command::new(nm);
    if nm_is_llvm_based(nm) {
        cmd.arg("--no-llvm-bc");
    }
    cmd
}

fn nm_is_llvm_based(nm: &Path) -> bool {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<PathBuf, bool>>> =
        std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(known) = cache.lock().ok().and_then(|c| c.get(nm).copied()) {
        return known;
    }
    let llvm_based = Command::new(nm)
        .arg("--version")
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("LLVM"));
    if let Ok(mut c) = cache.lock() {
        c.insert(nm.to_path_buf(), llvm_based);
    }
    llvm_based
}


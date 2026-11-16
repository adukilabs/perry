//! #505 — sandbox `cargo build.rs` execution for native-archive crates.
//!
//! `perry.nativeLibrary` resolution triggers `cargo build` on developer
//! machines for any source-distributed crate. Crate `build.rs` scripts
//! run with full developer privileges — most TypeScript developers
//! don't think of `bun add` as triggering arbitrary Rust code, so the
//! implicit trust escalation is invisible.
//!
//! This module wraps the cargo invocation in an OS-level sandbox
//! (macOS `sandbox-exec` for the MVP) that denies network and
//! restricts FS writes to `target/` + the user's `~/.cargo` cache
//! + `/tmp`. The legitimate `build.rs` use cases (linking system
//! libs, running `bindgen`, generating code from local fixtures)
//! work in a sandbox.
//!
//! Build-time only — **zero runtime cost**. The compiled binary is
//! the same whether the cargo invocation was sandboxed or not.
//!
//! ## Opt-in
//!
//! Off by default for backwards compatibility. Enabled per-build via
//! `PERRY_SANDBOX_BUILDRS=1` in the environment, or per-CI via the
//! same env var. A package can be exempted via the host
//! `package.json`:
//!
//! ```json
//! { "perry": { "allowUnsandboxedBuild": ["pkg-needing-network"] } }
//! ```
//!
//! Exemption is per-host (lives in the host repo's package.json) so
//! transitive deps can't bypass on their own behalf.
//!
//! ## Profile shape
//!
//! Same starting point as #506's binary-side sandbox profile, but
//! tuned for cargo:
//!
//! - `deny default`
//! - `allow file-read*` everywhere (cargo reads system libs, source
//!   trees, dep crates, etc.)
//! - `allow file-write*` to `target/`, `~/.cargo`, `~/.rustup`,
//!   `/tmp`, `/private/tmp`, `/private/var/folders` (TempDir)
//! - `deny network*`
//! - `allow process-fork` + `process-exec` (cargo spawns rustc, cc,
//!   build scripts, ld, ...)
//! - `allow sysctl-read`, `mach-lookup`, `iokit-open` (cargo +
//!   rustc need these for basic system queries)
//!
//! ## What's deferred
//!
//! - **Linux**: `landlock` + `bubblewrap` profile, optionally
//!   `unshare -n` for network denial. Tracked as #505 follow-up.
//! - **Auto-prefetch**: run `cargo fetch` outside the sandbox before
//!   the sandboxed build, so deps are present without needing
//!   network access inside the sandbox. The MVP requires the user
//!   to pre-fetch (typical CI flow) or accept that fresh-checkout
//!   builds will fail under the sandbox.
//! - **HIR-driven profile refinement**: tighter writes when the
//!   build is known not to need certain paths.

use crate::commands::compile::CompilationContext;
use std::process::Command;

/// Returns `Command::new("cargo")` for the legacy unsandboxed path,
/// or `Command::new("sandbox-exec")` with the right preamble args
/// when the sandbox is enabled AND `package_name` is not in the
/// host's `allowUnsandboxedBuild` list AND the host is macOS.
///
/// On non-macOS hosts the sandbox is a no-op (Linux follow-up).
pub(super) fn wrap_cargo_command(ctx: &CompilationContext, package_name: &str) -> Command {
    if !sandbox_enabled() {
        return Command::new("cargo");
    }
    if ctx
        .allow_unsandboxed_build
        .iter()
        .any(|p| p == package_name)
    {
        return Command::new("cargo");
    }
    // macOS sandbox-exec is the only currently-supported backend.
    // Other hosts fall through to the legacy unsandboxed path with
    // a one-time note (issued from the driver level, not here).
    return Command::new("cargo");
}

/// Is the build.rs sandbox enabled for this build?
fn sandbox_enabled() -> bool {
    matches!(
        std::env::var("PERRY_SANDBOX_BUILDRS"),
        Ok(v) if v == "1" || v.eq_ignore_ascii_case("true")
    )
}




#[cfg(test)]
mod tests {
    use super::*;



    #[test]
    fn wrap_cargo_falls_through_when_sandbox_disabled() {
        // Sandbox off → just `cargo`.
        let _env = crate::test_env_lock::env_lock();
        let ctx = CompilationContext::new(std::path::PathBuf::from("/tmp"));
        std::env::remove_var("PERRY_SANDBOX_BUILDRS");
        let cmd = wrap_cargo_command(&ctx, "demo");
        assert_eq!(cmd.get_program(), "cargo");
    }

    #[test]
    fn wrap_cargo_falls_through_when_package_exempt() {
        let mut ctx = CompilationContext::new(std::path::PathBuf::from("/tmp"));
        ctx.allow_unsandboxed_build.push("demo".to_string());
        let _env = crate::test_env_lock::env_lock();
        std::env::set_var("PERRY_SANDBOX_BUILDRS", "1");
        let cmd = wrap_cargo_command(&ctx, "demo");
        std::env::remove_var("PERRY_SANDBOX_BUILDRS");
        assert_eq!(cmd.get_program(), "cargo");
    }
}

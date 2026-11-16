//! WinUI 3 runtime-asset deployment for the executable link.
//!
//! Split out of `build_and_run.rs` (2000-line-per-file cap). Pure relocation
//! of the `windows-winui` bootstrap-import-library lookup and the runtime
//! asset copy that runs after a successful link, plus their tests.

use super::*;




pub(super) fn deploy_winui_runtime_assets(
) -> Result<()> {
    return Ok(());
}

#[cfg(test)]
mod winui_asset_tests {




}

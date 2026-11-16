//! Per-target Apple `.app` bundle writers — watchOS, tvOS, visionOS.
//!
//! Extracted from `compile.rs` for issue #1105 PR 3 (directory
//! split). The orchestrator's bundle dispatch reduces to a one-line
//! call per branch; per-platform Info.plist + asset-copy + i18n
//! logic lives here.
//!
//! iOS and macOS bundle writers remain in `compile.rs` for now —
//! their bodies are still inlined and need their own extraction PR.

use std::fs;
use std::path::{Path};


// Public-GM DT* (SDK / Xcode toolchain marker) fallbacks, used when neither a
// `PERRY_DT_*` env override nor the cross sysroot supplies the value (#4849).
// Verified GM toolchain on the build/sign worker (Xcode 26.3 / 17C529, SDK
// 26.2) via `xcrun --show-sdk-build-version`. DTXcode/DTXcodeBuild are one
// Xcode, so they're shared; the SDK *build* string differs per platform
// (tvOS 23K50 ≠ iOS 23C57 ≠ macOS 25C58), hence the per-platform table.



/// Read the human-facing app display name from `perry.toml`, used for the
/// `CFBundleDisplayName` Info.plist key — the name shown under the icon on the
/// Home Screen. Prefers the platform's own `[<platform>].display_name` (e.g.
/// `[ios]`, `[tvos]`), falling back to `[project].display_name`. Returns `None`
/// when neither is set, in which case the bundler omits CFBundleDisplayName and
/// the Home Screen falls back to CFBundleName — i.e. the sanitized executable
/// stem, e.g. `bloom-jump` instead of the intended `Bloom Jump`.
pub(super) fn read_app_display_name(input: &Path, platform: &str) -> Option<String> {
    let mut dir = input.canonicalize().ok()?;
    for _ in 0..5 {
        dir = dir.parent()?.to_path_buf();
        let toml_path = dir.join("perry.toml");
        if !toml_path.exists() {
            continue;
        }
        let doc: toml::Table = fs::read_to_string(&toml_path).ok()?.parse().ok()?;
        let from_table = |name: &str| {
            doc.get(name)
                .and_then(|v| v.as_table())
                .and_then(|t| t.get("display_name"))
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        // First perry.toml on the walk is the project manifest; stop here
        // whether or not it carries the key (mirrors read_apple_app_version).
        return from_table(platform).or_else(|| from_table("project"));
    }
    None
}

/// Minimal XML escape for text interpolated into an Info.plist `<string>`.
/// Display names are user-controlled (perry.toml), so an `&` or `<` would
/// otherwise produce a malformed plist that the bundle tooling rejects.
pub(super) fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}







#[cfg(test)]
mod tests {




}

//! Project-metadata + iOS signing-credential helpers for `perry run`.

use super::*;

/// Find project root by walking up from a directory
pub fn find_project_root(start: &Path) -> PathBuf {
    let mut dir = start.to_path_buf();
    for _ in 0..10 {
        if dir.join("package.json").exists() || dir.join("perry.toml").exists() {
            return dir;
        }
        if let Some(parent) = dir.parent() {
            dir = parent.to_path_buf();
        } else {
            break;
        }
    }
    start.to_path_buf()
}

/// Read app name and bundle ID from package.json
pub fn read_app_metadata(project_root: &Path, input: &Path) -> (String, String) {
    // Check perry.toml first (has [ios].bundle_id, [project].name)
    let toml_path = project_root.join("perry.toml");
    let toml_config = std::fs::read_to_string(&toml_path)
        .ok()
        .and_then(|s| toml::from_str::<toml::Value>(&s).ok());

    let toml_name = toml_config
        .as_ref()
        .and_then(|t| t.get("project"))
        .and_then(|p| p.get("name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let toml_bundle_id = toml_config
        .as_ref()
        .and_then(|t| {
            // Check [visionos].bundle_id, [ios].bundle_id, [macos].bundle_id, [app].bundle_id, [project].bundle_id, then top-level
            t.get("visionos")
                .and_then(|i| i.get("bundle_id"))
                .or_else(|| t.get("ios").and_then(|i| i.get("bundle_id")))
                .or_else(|| t.get("macos").and_then(|m| m.get("bundle_id")))
                .or_else(|| t.get("app").and_then(|a| a.get("bundle_id")))
                .or_else(|| t.get("project").and_then(|p| p.get("bundle_id")))
                .or_else(|| t.get("bundle_id"))
        })
        .and_then(|v| v.as_str())
        .map(|s| {
            // #999: this string flows into codesign argv when `perry run`
            // re-signs the iOS bundle. Validate before letting it through.
            let label = format!("perry.toml bundle_id at {}", toml_path.display());
            super::super::sanitize::validate_bundle_id_or_exit(s, &label)
        });

    // Then check package.json
    let pkg_path = project_root.join("package.json");
    let pkg = std::fs::read_to_string(&pkg_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());

    let pkg_name = pkg
        .as_ref()
        .and_then(|p| p.get("name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let pkg_bundle_id = pkg
        .as_ref()
        .and_then(|p| {
            p.get("bundleId")
                .or_else(|| p.get("perry").and_then(|pp| pp.get("bundleId")))
        })
        .and_then(|v| v.as_str())
        .map(|s| {
            // #999: validate before letting an explicit package.json
            // bundle ID reach codesign argv.
            let label = format!("package.json `bundleId` at {}", pkg_path.display());
            super::super::sanitize::validate_bundle_id_or_exit(s, &label)
        });

    let raw_name = toml_name.or(pkg_name).unwrap_or_else(|| {
        input
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("app")
            .to_string()
    });
    // Issue #500 (generalises #467): the package-derived name flows
    // into argv as `-o <name>` to the linker and as a bundle-ID stem
    // into codesign. Route through the shared sanitizer so every
    // hostile-character class (response-file `@`, flag-like `-`,
    // shell metacharacters, control bytes, path traversal, bidi
    // marks, …) is scrubbed at a single, fuzz-tested choke point.
    let name = super::super::sanitize::sanitize_for_linker_argv(&raw_name);

    let bundle_id = toml_bundle_id
        .or(pkg_bundle_id)
        .unwrap_or_else(|| format!("com.perry.{}", name));

    (name, bundle_id)
}

/// Read iOS-specific config from perry.toml
pub fn read_perry_toml_ios(project_root: &Path) -> Option<toml::Value> {
    let toml_path = project_root.join("perry.toml");
    let content = std::fs::read_to_string(&toml_path).ok()?;
    let config: toml::Value = content.parse().ok()?;
    config.get("ios").cloned()
}






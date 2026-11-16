//! i18n emit helpers: Android `res/values-*/strings.xml`, the
//! `.perry/i18n-keys.json` registry, and Apple `<bundle>/<locale>.lproj/
//! Localizable.strings` writers.
//!
//! Extracted from `compile.rs` for issue #1105 PR 3 (directory split).
//! Pure file move — no behavior change. Callers in the orchestrator
//! and in `bundle_for_visionos` (and, later, the iOS / macOS bundle
//! writers) share these routines for emitting i18n artifacts next to
//! the linked binary.

use std::fs;

use super::CompilationContext;

/// i18n: write the key registry to `.perry/i18n-keys.json` for tooling
/// (translators, key-extraction scripts) that needs a flat list of
/// emitted keys + their string-table indices. Best-effort.
pub(super) fn write_i18n_key_registry(
    ctx: &CompilationContext,
    i18n_table: Option<&perry_transform::i18n::I18nStringTable>,
) {
    let table = match i18n_table {
        Some(t) if !t.keys.is_empty() => t,
        _ => return,
    };
    let perry_dir = ctx.project_root.join(".perry");
    let _ = fs::create_dir_all(&perry_dir);
    let registry: Vec<serde_json::Value> = table
        .keys
        .iter()
        .enumerate()
        .map(|(i, key)| {
            serde_json::json!({
                "key": key,
                "string_idx": i,
            })
        })
        .collect();
    let registry_json = serde_json::json!({ "keys": registry });
    let _ = fs::write(
        perry_dir.join("i18n-keys.json"),
        serde_json::to_string_pretty(&registry_json).unwrap_or_default(),
    );
}



//! `[[widget]]` build glue for `perry compile --target {ios,android}`.
//!
//! Issue #676 (iOS) + #1139 (Android Glance). Walks the `perry.toml`
//! `[[widget]]` array and dispatches per platform:
//!
//! - **iOS**: [`build_declared_widgets_ios`] — invokes `swiftc` on
//!   `swift_source` and embeds the produced mach-O binary at
//!   `<.app>/Frameworks/<Name>.appex/<Name>` with a generated
//!   `Info.plist`.
//! - **Android**: [`build_declared_widgets_android`] — copies the
//!   `glance_source` Kotlin tree into the produced Gradle project at
//!   `app/src/main/java/<package>/widgets/<name>/`, emits
//!   `res/xml/widget_info_<name>.xml`, and injects a
//!   `<receiver>` entry into the merged `AndroidManifest.xml`.
//!
//! `watchos_source` is still skipped with a deprecation warning
//! (follow-up issue #676 watchOS slice).

use serde::Deserialize;


/// One `[[widget]]` entry in `perry.toml`. All optional except `name` so
/// the schema is forward-compatible — a user who only filled out the
/// iOS slot today doesn't have to revisit the manifest when the
/// watchOS / Android paths come online.
#[derive(Debug, Deserialize)]
pub struct WidgetEntry {
    pub name: String,
    /// Path to the SwiftUI source directory (relative to the perry.toml
    /// project root, or absolute). Required for the iOS build path; when
    /// omitted we treat the widget as "watchOS/Android only" and warn.
    pub swift_source: Option<String>,
    /// Optional watchOS variant. Not wired up in v1.
    pub watchos_source: Option<String>,
    /// Optional Android Glance variant. Not wired up in v1.
    pub glance_source: Option<String>,
    /// Issue #1179: TS entry point (relative to the perry.toml project
    /// root, or absolute) that declares one or more `Widget({...})` calls.
    /// When set, `perry compile --target ios` / `--target android`
    /// invokes the existing `--target ios-widget` / `--target android-widget`
    /// codegen internally and uses the produced sources in place of
    /// `swift_source` / `glance_source`. Lets apps keep a single source
    /// of truth (`widgets/index.ts`) without checking in generated code.
    pub ts_source: Option<String>,
    /// Display name shown in the widget gallery. Defaults to `name`.
    pub display_name: Option<String>,
    /// One-line description. Defaults to "{display_name} widget".
    pub description: Option<String>,
    /// Optional list of AppIntent type names the widget exposes. Unused
    /// by the build glue itself — recorded in the Info.plist as
    /// `NSExtensionPrincipalClass`-adjacent metadata in a follow-up
    /// (`#675` companion).
    #[serde(default)]
    pub intents: Vec<String>,
}

/// Top-level `[[widget]]` array parsed from `perry.toml`.
#[derive(Debug, Default, Deserialize)]
struct WidgetManifest {
    #[serde(default)]
    widget: Vec<WidgetEntry>,
}















#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parses_minimal_widget_entry() {
        let toml_text = r#"
[[widget]]
name = "TopSitesWidget"
swift_source = "ios-widgets/TopSitesWidget"
"#;
        let m: WidgetManifest = toml::from_str(toml_text).unwrap();
        assert_eq!(m.widget.len(), 1);
        assert_eq!(m.widget[0].name, "TopSitesWidget");
        assert_eq!(
            m.widget[0].swift_source.as_deref(),
            Some("ios-widgets/TopSitesWidget")
        );
    }

    #[test]
    fn manifest_parses_full_widget_entry() {
        let toml_text = r#"
[[widget]]
name = "DailyChangeWidget"
swift_source = "ios-widgets/DailyChangeWidget"
glance_source = "android-widgets/DailyChangeGlance"
display_name = "Daily change"
description = "Today's delta"
intents = ["DailyIntent", "RefreshIntent"]
"#;
        let m: WidgetManifest = toml::from_str(toml_text).unwrap();
        assert_eq!(m.widget.len(), 1);
        let w = &m.widget[0];
        assert_eq!(w.display_name.as_deref(), Some("Daily change"));
        assert_eq!(w.intents, vec!["DailyIntent", "RefreshIntent"]);
        assert!(w.glance_source.is_some());
    }

    #[test]
    fn manifest_parses_ts_source_widget_entry() {
        let toml_text = r#"
[[widget]]
name = "Widgets"
ts_source = "widgets/index.ts"
"#;
        let m: WidgetManifest = toml::from_str(toml_text).unwrap();
        assert_eq!(m.widget.len(), 1);
        let w = &m.widget[0];
        assert_eq!(w.name, "Widgets");
        assert_eq!(w.ts_source.as_deref(), Some("widgets/index.ts"));
        assert!(w.swift_source.is_none());
        assert!(w.glance_source.is_none());
    }









}

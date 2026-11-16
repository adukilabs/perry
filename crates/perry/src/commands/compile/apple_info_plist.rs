//! Apple-platform Info.plist mutators extracted from `compile.rs`.
//!
//! Two host-side post-codegen plist patches:
//!
//! - `inject_ios_deeplinks` (#583) — read `package.json` `perry.deepLinks` and
//!   inject CFBundleURLTypes into the generated Info.plist, plus write an
//!   `app.entitlements` for any `applinks:` associated domains.
//! - `inject_google_auth_info_plist` (#674/#1138) — read `[google_auth]` from
//!   `perry.toml` and inject `GIDClientID` / `GIDServerClientID` /
//!   `GIDDefaultScopes` for the `@perryts/google-auth` Swift bridge.
//! - `inject_ads_info_plist` (#867) — read `[ads]` from `perry.toml` and inject
//!   `GADApplicationIdentifier` (+ `NSUserTrackingUsageDescription`) for the
//!   Google Mobile Ads SDK.
//!
//! Both functions follow the same shape: return the mutated plist on success,
//! `None` on any read/parse/write failure. The orchestrator falls back to the
//! unmutated plist on `None` — matches the existing config-helper convention.











#[cfg(test)]
mod push_entitlement_tests {









}

#[cfg(test)]
mod ads_info_plist_tests {







}

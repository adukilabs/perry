//! HarmonyOS HAP (HarmonyOS Ability Package) bundler.
//!
//! Invoked by `compile` when `--target harmonyos[-simulator]` succeeds: takes
//! the already-linked `.so` + the ArkTS shim directory (both produced by
//! prior compile stages), assembles the OpenHarmony-required layout, optionally
//! compiles `.ets` → `.abc` via the SDK's ets-loader, zips the result as
//! `foo.hap`, and optionally runs `hap-sign` with user-supplied credentials.
//!
//! All signing credentials come from env vars — no config file in v1. The
//! split into P12 keystore + cert chain + profile mirrors
//! `hap-sign-tool.jar`'s `sign-app` CLI (README lines 297-314 of
//! developtools_hapsigner); B.3 conflated cert chain with profile and had
//! to be patched in B.4 when that was caught by audit.
//!
//!   PERRY_HARMONYOS_P12            — path to signing key (.p12)
//!   PERRY_HARMONYOS_P12_PASSWORD   — password for the .p12 (keystore)
//!   PERRY_HARMONYOS_CERT           — path to app cert chain (.cer / .pem);
//!                                     DevEco auto-signing names it
//!                                     `<bundleName>.cer`. Distinct from
//!                                     PROFILE.
//!   PERRY_HARMONYOS_PROFILE        — path to signed provisioning profile
//!                                     (.p7b). DevEco names it
//!                                     `<bundleName>.p7b`.
//!   PERRY_HARMONYOS_KEY_ALIAS      — alias inside the .p12 (defaults to
//!                                     "debugKey", DevEco's convention).
//!   PERRY_HARMONYOS_KEY_PASSWORD   — private-key password (often the same
//!                                     as the keystore password; defaults
//!                                     to PERRY_HARMONYOS_P12_PASSWORD).
//!   PERRY_HARMONYOS_SIGN_ALG       — SHA256withECDSA | SHA384withECDSA
//!                                     (default SHA256withECDSA).
//!   PERRY_HARMONYOS_BUNDLE_NAME    — must match the cert's bundle name.
//!                                     Falls back to `com.perry.app.<stem>`.
//!   PERRY_HARMONYOS_HAPSIGN        — override path to hap-sign-tool.jar;
//!                                     default: <sdk>/toolchains/lib/hap-sign-tool.jar
//!                                     (invoked via `java -jar ...`).
//!
//! If PERRY_HARMONYOS_P12, _P12_PASSWORD, _CERT, or _PROFILE is unset, the
//! HAP is emitted unsigned (`<stem>.unsigned.hap`) with a remediation
//! message. An unsigned HAP won't install via `hdc install`.
//!
//! Alternative path (used during v0.5.129 first-emulator validation):
//! copy the `.so` + `.ets` Perry emits into a DevEco Studio project and
//! let DevEco's hvigor do the signing & install. Sidesteps the env-var
//! dance entirely, at the cost of a two-step workflow. See the v0.5.129
//! CLAUDE.md entry.










#[cfg(test)]
mod tests {


}









#[cfg(test)]
mod build_hap_tests {



}


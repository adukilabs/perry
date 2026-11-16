use super::*;

/// Prompt for target platform selection.
pub(super) fn prompt_target(default: Option<&str>) -> String {
    let options = &[
        "macOS", "iOS", "visionOS", "tvOS", "watchOS", "Android", "Linux",
    ];
    let default_idx = match default {
        Some("ios") => 1,
        Some("visionos") => 2,
        Some("tvos") => 3,
        Some("watchos") => 4,
        Some("android") => 5,
        Some("linux") => 6,
        _ => 0,
    };
    let selection = Select::new()
        .with_prompt("Target platform")
        .items(options)
        .default(default_idx)
        .interact()
        .unwrap_or(0);
    match selection {
        1 => "ios".into(),
        2 => "visionos".into(),
        3 => "tvos".into(),
        4 => "watchos".into(),
        5 => "android".into(),
        6 => "linux".into(),
        _ => "macos".into(),
    }
}

/// Resolve a credential value using priority: CLI flag → env var → saved config → interactive prompt.
/// Returns None only if the field is optional and the user skips it.
pub(super) fn resolve_credential(
    cli_value: Option<&str>,
    env_var: &str,
    saved_value: Option<&str>,
    prompt_label: &str,
    required: bool,
    interactive: bool,
) -> Option<String> {
    // 1. CLI flag
    if let Some(v) = cli_value {
        if !v.is_empty() {
            return Some(v.to_string());
        }
    }
    // 2. Environment variable
    if let Ok(v) = std::env::var(env_var) {
        if !v.is_empty() {
            return Some(v);
        }
    }
    // 3. Saved config
    if let Some(v) = saved_value {
        if !v.is_empty() {
            return Some(v.to_string());
        }
    }
    // 4. Interactive prompt
    if interactive {
        let val = prompt_input(prompt_label, saved_value);
        if val.is_some() {
            return val;
        }
        if required {
            // Re-prompt once if required
            return prompt_input(&format!("{prompt_label} (required)"), None);
        }
    }
    None
}

/// Auto-detect a signing identity from the macOS Keychain and export it as .p12.
/// Returns (base64_p12_data, password) or None if not on macOS, no identity found, or user declines.
pub(super) fn auto_export_p12_from_keychain(
    configured_identity: Option<&str>,
    interactive: bool,
) -> Option<(String, String)> {
    return None;

    use perry_base64::Engine;
}

/// Resolve a file path credential: CLI → env → saved config → interactive prompt.
/// Returns the path string (not validated here).
pub(super) fn resolve_path_credential(
    cli_value: Option<&Path>,
    env_var: &str,
    saved_value: Option<&str>,
    prompt_label: &str,
    interactive: bool,
) -> Option<String> {
    if let Some(v) = cli_value {
        return Some(v.to_string_lossy().to_string());
    }
    if let Ok(v) = std::env::var(env_var) {
        if !v.is_empty() {
            return Some(v);
        }
    }
    if let Some(v) = saved_value {
        if !v.is_empty() {
            return Some(v.to_string());
        }
    }
    if interactive {
        return prompt_input(prompt_label, saved_value);
    }
    None
}

/// Validate that required distribution credentials are present before starting a build.
///
/// Called immediately after credential resolution, before the tarball is created,
/// so users get a clear error message without waiting for a full build to complete.
pub(super) fn validate_credentials_for_distribute(
    is_android: bool,
    android_distribute: Option<&str>,
    google_play_json: Option<&str>,
    is_ios: bool,
    ios_distribute: Option<&str>,
    apple_key_id: Option<&str>,
    apple_issuer_id: Option<&str>,
    p8_key_content: Option<&str>,
    is_macos: bool,
    macos_distribute: Option<&str>,
    is_tvos: bool,
    tvos_distribute: Option<&str>,
    is_watchos: bool,
    watchos_distribute: Option<&str>,
) -> Result<()> {
    // Android + playstore
    if is_android {
        let distribute = android_distribute.unwrap_or("");
        if distribute == "playstore" || distribute.starts_with("playstore:") {
            if google_play_json.is_none() {
                bail!(
                    "android.distribute = \"playstore\" requires a Google Play service account JSON key.\n\
                     Run `perry setup android`, pass --google-play-key <path>, or set PERRY_GOOGLE_PLAY_KEY_PATH.\n\
                     To build without uploading, remove distribute = \"playstore\" from perry.toml."
                );
            }
            // Validate track if explicitly specified
            if let Some(track) = distribute.strip_prefix("playstore:") {
                if !matches!(track, "internal" | "alpha" | "beta" | "production") {
                    bail!(
                        "Invalid Play Store track \"{track}\". Valid values: internal, alpha, beta, production.\n\
                         Example: distribute = \"playstore:beta\""
                    );
                }
            }
        }
    }

    // iOS + appstore/testflight
    if is_ios {
        let distribute = ios_distribute.unwrap_or("");
        if distribute == "appstore" || distribute == "testflight" {
            let mut missing = Vec::new();
            if apple_key_id.is_none() {
                missing.push("Key ID (--apple-key-id / PERRY_APPLE_KEY_ID)");
            }
            if apple_issuer_id.is_none() {
                missing.push("Issuer ID (--apple-issuer-id / PERRY_APPLE_ISSUER_ID)");
            }
            if p8_key_content.is_none() {
                missing.push(".p8 key (--apple-p8-key / PERRY_APPLE_P8_KEY)");
            }
            if !missing.is_empty() {
                bail!(
                    "ios.distribute = \"{distribute}\" requires App Store Connect API credentials.\n\
                     Missing: {}\n\
                     Run `perry setup ios` or pass the missing flags.",
                    missing.join(", ")
                );
            }
        }
    }

    // tvOS + appstore/testflight (signs/packages exactly like iOS)
    if is_tvos {
        let distribute = tvos_distribute.unwrap_or("");
        if distribute == "appstore" || distribute == "testflight" {
            let mut missing = Vec::new();
            if apple_key_id.is_none() {
                missing.push("Key ID (--apple-key-id / PERRY_APPLE_KEY_ID)");
            }
            if apple_issuer_id.is_none() {
                missing.push("Issuer ID (--apple-issuer-id / PERRY_APPLE_ISSUER_ID)");
            }
            if p8_key_content.is_none() {
                missing.push(".p8 key (--apple-p8-key / PERRY_APPLE_P8_KEY)");
            }
            if !missing.is_empty() {
                bail!(
                    "tvos.distribute = \"{distribute}\" requires App Store Connect API credentials.\n\
                     Missing: {}\n\
                     Run `perry setup tvos` or pass the missing flags.",
                    missing.join(", ")
                );
            }
        }
    }

    // watchOS + appstore/testflight (standalone watch app, uploads like iOS)
    if is_watchos {
        let distribute = watchos_distribute.unwrap_or("");
        if distribute == "appstore" || distribute == "testflight" {
            let mut missing = Vec::new();
            if apple_key_id.is_none() {
                missing.push("Key ID (--apple-key-id / PERRY_APPLE_KEY_ID)");
            }
            if apple_issuer_id.is_none() {
                missing.push("Issuer ID (--apple-issuer-id / PERRY_APPLE_ISSUER_ID)");
            }
            if p8_key_content.is_none() {
                missing.push(".p8 key (--apple-p8-key / PERRY_APPLE_P8_KEY)");
            }
            if !missing.is_empty() {
                bail!(
                    "watchos.distribute = \"{distribute}\" requires App Store Connect API credentials.\n\
                     Missing: {}\n\
                     Run `perry setup watchos` or pass the missing flags.",
                    missing.join(", ")
                );
            }
        }
    }

    // macOS + appstore/notarize/both
    if is_macos {
        let distribute = macos_distribute.unwrap_or("");
        if matches!(distribute, "appstore" | "testflight" | "notarize" | "both") {
            let mut missing = Vec::new();
            if apple_key_id.is_none() {
                missing.push("Key ID (--apple-key-id / PERRY_APPLE_KEY_ID)");
            }
            if apple_issuer_id.is_none() {
                missing.push("Issuer ID (--apple-issuer-id / PERRY_APPLE_ISSUER_ID)");
            }
            if p8_key_content.is_none() {
                missing.push(".p8 key (--apple-p8-key / PERRY_APPLE_P8_KEY)");
            }
            if !missing.is_empty() {
                let purpose = match distribute {
                    "notarize" => "notarization",
                    "both" => "App Store upload and notarization",
                    "appstore" | "testflight" => "App Store Connect upload",
                    _ => "distribution",
                };
                bail!(
                    "macos.distribute = \"{distribute}\" requires App Store Connect API credentials for {purpose}.\n\
                     Missing: {}\n\
                     Run `perry setup macos` or pass the missing flags.",
                    missing.join(", ")
                );
            }
        }
    }

    Ok(())
}

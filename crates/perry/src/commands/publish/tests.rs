#![cfg(test)]

use super::*;
use std::fs;

#[test]
fn test_perry_config_roundtrip() {
    let config = PerryConfig {
        license_key: Some("FREE-abc123".into()),
        server: Some("https://build.example.com".into()),
        default_target: Some("macos".into()),
        apple: Some(AppleSavedConfig {
            team_id: Some("ABC123DEF".into()),
            p8_key_path: Some("/Users/me/AuthKey_XXX.p8".into()),
            key_id: Some("XXX".into()),
            issuer_id: Some("abc-def-ghi".into()),
        }),
        ios: Some(IosSavedConfig {}),
        android: Some(AndroidSavedConfig {
            keystore_path: Some("/Users/me/release.keystore".into()),
            key_alias: Some("key0".into()),
            google_play_key_path: Some("/Users/me/play-sa.json".into()),
        }),
        ..Default::default()
    };

    let toml_str = toml::to_string_pretty(&config).unwrap();
    let parsed: PerryConfig = toml::from_str(&toml_str).unwrap();

    assert_eq!(parsed.license_key, config.license_key);
    assert_eq!(parsed.server, config.server);
    assert_eq!(parsed.default_target, config.default_target);
    assert_eq!(
        parsed.apple.as_ref().unwrap().team_id,
        config.apple.as_ref().unwrap().team_id
    );
    assert_eq!(
        parsed.android.as_ref().unwrap().google_play_key_path,
        config.android.as_ref().unwrap().google_play_key_path
    );
}

#[test]
fn test_perry_config_minimal() {
    let config = PerryConfig {
        license_key: Some("FREE-test".into()),
        ..Default::default()
    };
    let toml_str = toml::to_string_pretty(&config).unwrap();
    assert!(toml_str.contains("license_key"));
    assert!(
        !toml_str.contains("[apple]"),
        "empty sections should be omitted"
    );
    assert!(!toml_str.contains("[android]"));
}

#[test]
fn test_perry_config_parse_legacy_format() {
    // Old format was just license_key = "..." — should still parse
    let legacy = r#"license_key = "FREE-legacy-key""#;
    let config: PerryConfig = toml::from_str(legacy).unwrap();
    assert_eq!(config.license_key.as_deref(), Some("FREE-legacy-key"));
    assert!(config.apple.is_none());
    assert!(config.default_target.is_none());
}

#[test]
fn test_perry_config_no_passwords_in_toml() {
    let config = PerryConfig {
        license_key: Some("FREE-test".into()),
        android: Some(AndroidSavedConfig {
            keystore_path: Some("/path/to/keystore".into()),
            key_alias: Some("key0".into()),
            google_play_key_path: None,
        }),
        ..Default::default()
    };
    let toml_str = toml::to_string_pretty(&config).unwrap();
    // Config struct intentionally has no password fields
    assert!(!toml_str.contains("password"));
    assert!(toml_str.contains("keystore_path"));
}

#[test]
fn test_perry_config_default_is_empty() {
    let config = PerryConfig::default();
    assert!(config.license_key.is_none());
    assert!(config.server.is_none());
    assert!(config.default_target.is_none());
    assert!(config.apple.is_none());
    assert!(config.ios.is_none());
    assert!(config.android.is_none());
}

#[test]
fn test_credentials_payload_with_google_play() {
    let creds = CredentialsPayload {
        apple_team_id: None,
        apple_signing_identity: None,
        apple_key_id: None,
        apple_issuer_id: None,
        apple_p8_key: None,
        provisioning_profile_base64: None,
        apple_certificate_p12_base64: None,
        apple_certificate_password: None,
        apple_notarize_certificate_p12_base64: None,
        apple_notarize_certificate_password: None,
        apple_notarize_signing_identity: None,
        apple_installer_certificate_p12_base64: None,
        apple_installer_certificate_password: None,
        android_keystore_base64: Some("dGVzdA==".into()),
        android_keystore_password: Some("pass".into()),
        android_key_alias: Some("key0".into()),
        android_key_password: None,
        google_play_service_account_json: Some("{\"client_email\":\"test@gcp\"}".into()),
        gcloud_kms_key: None,
        gcloud_kms_cert_base64: None,
        gcloud_service_account_base64: None,
    };
    let json = serde_json::to_string(&creds).unwrap();
    assert!(json.contains("google_play_service_account_json"));
    assert!(json.contains("client_email"));
}

#[test]
fn test_credentials_payload_omits_none() {
    let creds = CredentialsPayload {
        apple_team_id: Some("ABC".into()),
        apple_signing_identity: Some("Dev ID".into()),
        apple_key_id: None,
        apple_issuer_id: None,
        apple_p8_key: None,
        provisioning_profile_base64: None,
        apple_certificate_p12_base64: None,
        apple_certificate_password: None,
        apple_notarize_certificate_p12_base64: None,
        apple_notarize_certificate_password: None,
        apple_notarize_signing_identity: None,
        apple_installer_certificate_p12_base64: None,
        apple_installer_certificate_password: None,
        android_keystore_base64: None,
        android_keystore_password: None,
        android_key_alias: None,
        android_key_password: None,
        google_play_service_account_json: None,
        gcloud_kms_key: None,
        gcloud_kms_cert_base64: None,
        gcloud_service_account_base64: None,
    };
    let json = serde_json::to_string(&creds).unwrap();
    // Fields with skip_serializing_if should be absent
    assert!(!json.contains("android_keystore_base64"));
    assert!(!json.contains("google_play_service_account_json"));
    // Non-skip fields are always present (even as null)
    assert!(json.contains("apple_team_id"));
}







#[test]
fn test_config_file_write_and_read() {
    // Test writing to a temp location and reading back
    let dir = std::env::temp_dir().join("perry-test-config");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("config.toml");

    let config = PerryConfig {
        license_key: Some("TEST-KEY-123".into()),
        default_target: Some("ios".into()),
        apple: Some(AppleSavedConfig {
            team_id: Some("TEAM123".into()),
            ..Default::default()
        }),
        ..Default::default()
    };

    let content = toml::to_string_pretty(&config).unwrap();
    fs::write(&path, &content).unwrap();

    let read_back = fs::read_to_string(&path).unwrap();
    let parsed: PerryConfig = toml::from_str(&read_back).unwrap();
    assert_eq!(parsed.license_key.as_deref(), Some("TEST-KEY-123"));
    assert_eq!(parsed.default_target.as_deref(), Some("ios"));
    assert_eq!(parsed.apple.unwrap().team_id.as_deref(), Some("TEAM123"));

    // Cleanup
    let _ = fs::remove_file(&path);
    let _ = fs::remove_dir(&dir);
}

// The trailing `false, None, false, None` on each call is
// (is_tvos, tvos_distribute, is_watchos, watchos_distribute) — not applicable
// to the android/ios/macos cases below. tvOS/watchOS have dedicated tests.















// --- GHSA-x55v-q459-68ch: server-controlled artifact path handling ---





use super::*;

// --- Server API types ---

// #854: deserialized server response — full wire shape kept even where a
// field isn't consumed on the client path.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(super) struct RegisterResponse {
    pub(super) license_key: String,
    pub(super) tier: String,
    pub(super) platforms: Vec<String>,
}

// #854: deserialized server response — full wire shape kept.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(super) struct BuildResponse {
    pub(super) job_id: String,
    pub(super) ws_url: String,
    pub(super) position: usize,
}

// #854: full server-message protocol — every variant/field is part of the
// deserialized wire contract; some payload fields aren't consumed yet.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ServerMessage {
    JobCreated {
        job_id: String,
        position: usize,
        estimated_wait_secs: Option<u64>,
    },
    QueueUpdate {
        position: usize,
        estimated_wait_secs: Option<u64>,
    },
    Stage {
        stage: String,
        message: String,
    },
    Log {
        stage: String,
        line: String,
        stream: String,
    },
    Progress {
        stage: String,
        percent: u8,
        message: Option<String>,
    },
    ArtifactReady {
        artifact_name: String,
        artifact_size: u64,
        sha256: String,
        download_url: String,
        expires_in_secs: u64,
        #[serde(default)]
        download_path: Option<String>,
    },
    Published {
        platform: String,
        message: String,
        url: Option<String>,
    },
    Error {
        code: String,
        message: String,
        stage: Option<String>,
    },
    Complete {
        job_id: String,
        success: bool,
        duration_secs: f64,
        artifacts: Vec<serde_json::Value>,
    },
}

// --- Manifest sent to the build server ---


#[derive(Debug, Serialize)]
pub(super) struct CredentialsPayload {
    pub(super) apple_team_id: Option<String>,
    pub(super) apple_signing_identity: Option<String>,
    pub(super) apple_key_id: Option<String>,
    pub(super) apple_issuer_id: Option<String>,
    pub(super) apple_p8_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) provisioning_profile_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) apple_certificate_p12_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) apple_certificate_password: Option<String>,
    /// For macOS distribute = "both": separate Developer ID cert for notarization
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) apple_notarize_certificate_p12_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) apple_notarize_certificate_password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) apple_notarize_signing_identity: Option<String>,
    /// Separate .p12 for the Mac Installer Distribution cert (for .pkg signing)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) apple_installer_certificate_p12_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) apple_installer_certificate_password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) android_keystore_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) android_keystore_password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) android_key_alias: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) android_key_password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) google_play_service_account_json: Option<String>,
    /// Google Cloud KMS key path for Windows code signing
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) gcloud_kms_key: Option<String>,
    /// Base64-encoded code signing certificate for GCloud KMS
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) gcloud_kms_cert_base64: Option<String>,
    /// Base64-encoded GCP service account JSON key
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) gcloud_service_account_base64: Option<String>,
}

/// Claim an anonymous licence key from the hub.
///
/// Lives here rather than in `mod.rs` because it is a self-contained hub call
/// with no publish state, and `mod.rs` is at the 2000-line cap.
pub(crate) fn auto_register_license(server_url: &str) -> Result<String> {
    let client = perry_http_client::Client::new();
    let resp = client
        .execute(
            perry_http_client::Request::post(format!("{server_url}/api/v1/license/register"))
                .json_body("{}".into()),
        )
        .context("Failed to register license")?;
    if !resp.is_success() {
        let body = resp.text();
        bail!("License registration failed: {body}");
    }
    let reg: RegisterResponse = serde_json::from_slice(&resp.body)?;
    Ok(reg.license_key)
}

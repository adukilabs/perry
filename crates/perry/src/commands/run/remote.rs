//! Remote build via Perry Hub: tarball upload, WebSocket progress, IPA extraction.

use super::*;

/// Build remotely via Perry Hub and launch the result.
///
/// Synchronous since P11: the HTTP upload, the artifact download and the
/// progress WebSocket all run on `perry_http_client`, which owns a
/// `turnloop::Loop` per connection and turns it to completion. There is no JS
/// event loop on this thread and nothing else for it to do while the build
/// runs, so a runtime bought nothing.
pub fn remote_build_and_launch(
    input: &Path,
    target: &str,
    device_udid: Option<&str>,
    program_args: &[String],
    enable_geisterhand: bool,
    geisterhand_port: Option<u16>,
    format: OutputFormat,
) -> Result<()> {
    use super::super::publish::{
        auto_register_license, create_project_tarball_with_filters, load_config, save_config,
    };
    use crate::terminal_progress::ProgressBar;
    use perry_base64::Engine;
    use perry_http_client::ws::Message;
    use perry_http_client::{Form, Request, WebSocket};
    use serde::Deserialize;
    use std::io::Write;

    let project_dir = input
        .parent()
        .unwrap_or(Path::new("."))
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from("."));

    // Walk up to find project root (directory containing package.json or perry.toml)
    let project_root = find_project_root(&project_dir);

    // Resolve server URL and license key
    let mut config = load_config();
    let server_url = config
        .server
        .clone()
        .unwrap_or_else(|| "https://hub.perryts.com".into());

    let license_key = match config.license_key.clone() {
        Some(key) => key,
        None => {
            if let OutputFormat::Text = format {
                println!("  Registering with Perry Hub...");
            }
            let key = auto_register_license(&server_url)?;
            config.license_key = Some(key.clone());
            save_config(&config)?;
            key
        }
    };

    // Determine app name and bundle ID from package.json
    let (app_name, bundle_id) = read_app_metadata(&project_root, input);

    // Determine entry path relative to project root
    let entry = input
        .canonicalize()
        .unwrap_or_else(|_| input.to_path_buf())
        .strip_prefix(&project_root)
        .unwrap_or(input)
        .to_string_lossy()
        .to_string();

    // The build target for the manifest
    let build_target = match target {
        other => other,
    };

    if let OutputFormat::Text = format {
        println!();
        println!(
            "  {} Building {} for {} via Perry Hub",
            style("▶").cyan().bold(),
            style(&app_name).bold(),
            style(target).cyan()
        );
        println!();
    }

    // Package project
    if let OutputFormat::Text = format {
        print!("  Packaging project...");
        std::io::stdout().flush().ok();
    }

    // Read [publish] filters so remote runs use the same archive policy as publish.
    let (publish_excludes, publish_includes) =
        std::fs::read_to_string(project_root.join("perry.toml"))
            .ok()
            .and_then(|s| toml::from_str::<toml::Value>(&s).ok())
            .and_then(|v| v.get("publish").cloned())
            .map(|publish| {
                let strings = |key: &str| {
                    publish
                        .get(key)
                        .and_then(|value| value.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|value| value.as_str().map(String::from))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                };
                (strings("exclude"), strings("include"))
            })
            .unwrap_or_default();
    let tarball =
        create_project_tarball_with_filters(&project_root, &publish_excludes, &publish_includes)
            .context("Failed to create project tarball")?;

    if let OutputFormat::Text = format {
        println!(
            " {} ({:.1} MB)",
            style("done").green(),
            tarball.len() as f64 / 1_048_576.0
        );
    }

    // Build manifest
    let ios_distribute = "none";
    let manifest = serde_json::json!({
        "app_name": app_name,
        "bundle_id": bundle_id,
        "version": "0.0.1",
        "entry": entry,
        "targets": [build_target],
        "ios_distribute": ios_distribute,
        "enable_geisterhand": enable_geisterhand,
        "geisterhand_port": geisterhand_port.unwrap_or(7676),
    });

    // Build credentials — device builds need signing
    let ios_toml = read_perry_toml_ios(&project_root);
    let credentials = serde_json::json!({
        "apple_team_id": null,
        "apple_signing_identity": null,
        "apple_key_id": null,
        "apple_issuer_id": null,
        "apple_p8_key": null
    });

    // Upload
    if let OutputFormat::Text = format {
        print!("  Uploading to build server...");
        std::io::stdout().flush().ok();
    }

    let tarball_b64 = perry_base64::engine::general_purpose::STANDARD.encode(&tarball);

    // `reqwest::Client::new()` had no timeout here and a remote build upload is
    // a whole project tarball, so the budget is generous rather than default.
    let client = perry_http_client::Client::with_timeout(std::time::Duration::from_secs(900));
    let form = Form::new()
        .text("license_key", license_key)
        .text("manifest", serde_json::to_string(&manifest)?)
        .text("credentials", serde_json::to_string(&credentials)?)
        .text("tarball_b64", tarball_b64);

    let resp = client
        .post_form(&format!("{server_url}/api/v1/build"), form)
        .context("Failed to connect to build server")?;

    if !resp.is_success() {
        let status = resp.status;
        let body = resp.text();
        bail!("Build server returned {status}: {body}");
    }

    #[derive(Deserialize)]
    struct BuildResponse {
        job_id: String,
        ws_url: String,
        position: usize,
    }

    let build_resp: BuildResponse =
        serde_json::from_slice(&resp.body).context("Invalid build response")?;

    if let OutputFormat::Text = format {
        println!(" {}", style("done").green());
        println!("  Job ID:    {}", style(&build_resp.job_id).dim());
        if build_resp.position > 1 {
            println!("  Position:  {}", build_resp.position);
        }
        println!();
    }

    // WebSocket progress
    let ws_url =
        if build_resp.ws_url.starts_with("ws://") || build_resp.ws_url.starts_with("wss://") {
            build_resp.ws_url.clone()
        } else if server_url.starts_with("https://") {
            format!(
                "wss://{}{}",
                &server_url["https://".len()..],
                build_resp.ws_url
            )
        } else {
            format!(
                "ws://{}{}",
                &server_url["http://".len()..],
                build_resp.ws_url
            )
        };

    // A remote build can queue for a long time before the first frame, so the
    // per-message read budget is the same 15 minutes the upload gets. The
    // connect itself is bounded separately and much tighter.
    //
    // This IS a behaviour change and it is the drift worth knowing about: the
    // async stream waited forever, so a hub that went quiet mid-build hung the
    // command; it now fails after 15 minutes of silence, and — unlike
    // `perry publish`, which has a `reconnect_or_bail!` loop — there is no
    // reconnect here to fall back on. Both shapes are bad; which is worse is a
    // product call, and this one at least terminates. See
    // docs/turnloop/p11-report.md, "Behaviour changes".
    let ws_read_timeout = std::time::Duration::from_secs(900);
    let mut ws = WebSocket::connect(&ws_url, std::time::Duration::from_secs(30))
        .context("Failed to connect WebSocket")?;

    ws.send_text(
        &format!(r#"{{"type":"subscribe","job_id":"{}"}}"#, build_resp.job_id),
        std::time::Duration::from_secs(30),
    )
    .context("Failed to send subscribe message")?;

    let pb = if let OutputFormat::Text = format {
        let pb = ProgressBar::new(100);
        pb.set_message("Waiting for build...");
        Some(pb)
    } else {
        None
    };

    #[derive(Debug, Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum ServerMsg {
        JobCreated {
            #[serde(default)]
            // #854: part of the deserialized JobCreated wire shape; not read.
            #[allow(dead_code)]
            job_id: Option<String>,
        },
        QueueUpdate {
            position: usize,
        },
        Stage {
            #[allow(dead_code)]
            stage: String,
            message: String,
        },
        Log {
            line: String,
            stream: String,
        },
        Progress {
            percent: u8,
        },
        ArtifactReady {
            artifact_name: String,
            download_url: String,
            #[serde(default)]
            download_path: Option<String>,
        },
        Published {
            #[allow(dead_code)]
            platform: String,
            #[allow(dead_code)]
            message: String,
        },
        Error {
            message: String,
        },
        #[serde(other)]
        Unknown,
    }

    let mut download_url: Option<String> = None;
    let mut download_path: Option<String> = None;
    let mut artifact_name: Option<String> = None;
    let mut build_success = false;

    loop {
        let msg = match ws.read_message(ws_read_timeout) {
            Ok(Some(m)) => m,
            // `None` is the peer closing — the same thing `Message::Close`
            // meant before, and the same thing the stream ending meant.
            Ok(None) => break,
            Err(e) => {
                if let Some(ref pb) = pb {
                    pb.abandon_with_message(format!("WebSocket error: {e}"));
                }
                bail!("WebSocket error: {e}");
            }
        };

        let text = match msg {
            Message::Text(t) => t,
            Message::Close(_) => break,
            Message::Binary(_) => continue,
        };

        let server_msg: ServerMsg = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(_) => continue,
        };

        match server_msg {
            ServerMsg::JobCreated { .. } => {
                if let Some(ref pb) = pb {
                    pb.set_message("Build started");
                }
            }
            ServerMsg::QueueUpdate { position, .. } => {
                if let Some(ref pb) = pb {
                    pb.set_message(format!("Queue position: {position}"));
                }
            }
            ServerMsg::Stage { message, .. } => {
                if let Some(ref pb) = pb {
                    pb.set_message(format!("▶️  {message}"));
                }
            }
            ServerMsg::Log { line, stream, .. } => {
                if let Some(ref pb) = pb {
                    if stream == "stderr" {
                        pb.println(format!("    {}", style(&line).dim()));
                    }
                }
            }
            ServerMsg::Progress { percent, .. } => {
                if let Some(ref pb) = pb {
                    pb.set_position(percent as u64);
                }
            }
            ServerMsg::ArtifactReady {
                artifact_name: name,
                download_url: url,
                download_path: path,
                ..
            } => {
                if let Some(ref pb) = pb {
                    pb.set_position(100);
                    pb.finish_with_message(format!("✓ Build complete: {}", style(&name).bold()));
                }
                download_url = Some(url);
                download_path = path;
                artifact_name = Some(name);
                build_success = true;
                break; // artifact ready, proceed to download
            }
            ServerMsg::Published { .. } => {
                build_success = true;
                break;
            }
            ServerMsg::Error { message, .. } => {
                if let Some(ref pb) = pb {
                    pb.abandon_with_message(format!("✗ {message}"));
                }
                bail!("Build failed: {message}");
            }
            ServerMsg::Unknown => {}
        }
    }

    if !build_success {
        bail!("Build failed (no artifact received)");
    }

    // Download artifact
    let (url, name) = match (download_url, artifact_name) {
        (Some(u), Some(n)) => (u, n),
        _ => bail!("Build succeeded but no download URL received"),
    };

    if let OutputFormat::Text = format {
        print!("  Downloading {}...", name);
        std::io::stdout().flush().ok();
    }

    let dist_dir = PathBuf::from("dist");
    std::fs::create_dir_all(&dist_dir)?;
    let dest = dist_dir.join(&name);

    if let Some(ref src_path) = download_path {
        std::fs::copy(src_path, &dest)
            .with_context(|| format!("Failed to copy artifact from {src_path}"))?;
    } else {
        let full_url = if url.starts_with("http://") || url.starts_with("https://") {
            url.clone()
        } else {
            format!("{server_url}{url}")
        };
        // The artifact is buffered rather than streamed, exactly as before —
        // the base64 sniff below needs the whole body anyway. The default
        // 32 MiB ceiling would refuse a real app bundle, so it is raised here
        // rather than in the default.
        let resp = client
            .clone()
            .max_body(2 * 1024 * 1024 * 1024)
            .execute(Request::get(&full_url))
            .context("Failed to download artifact")?;

        if !resp.is_success() {
            bail!("Download failed: {}", resp.status);
        }

        let bytes = resp.body;
        // Detect base64-encoded content
        let data = if bytes.len() > 4
            && bytes.iter().all(|&b| {
                b.is_ascii_alphanumeric()
                    || b == b'+'
                    || b == b'/'
                    || b == b'='
                    || b == b'\n'
                    || b == b'\r'
            }) {
            perry_base64::engine::general_purpose::STANDARD
                .decode(&bytes)
                .unwrap_or(bytes)
        } else {
            bytes
        };
        std::fs::write(&dest, &data)?;
    }

    if let OutputFormat::Text = format {
        println!(
            " {} → {}",
            style("done").green(),
            style(dest.display()).bold()
        );
        println!();
    }

    // For iOS: extract .app from .ipa and install
    // Native binary
    launch_native(&dest, program_args, format)
}






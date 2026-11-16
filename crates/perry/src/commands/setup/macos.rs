use anyhow::{Result};
use std::process::Command;




/// Merge two .p12 files into the first one (appends the second's cert+key).
/// Both must use the same password. Uses openssl to extract PEM and repackage.
// #854: signing helper for the macOS distribute="both" cert-merge flow;
// kept as the documented entry point even where currently unreferenced.
#[allow(dead_code)]
pub fn merge_p12_files(
    primary_p12: &std::path::Path,
    secondary_p12: &str,
    password: &str,
    tmpdir: &std::path::Path,
) -> Result<()> {
    let pass = format!("pass:{password}");
    let pem_a = tmpdir.join("_merge_a.pem");
    let pem_b = tmpdir.join("_merge_b.pem");
    let combined = tmpdir.join("_merge_combined.pem");

    // Extract both to PEM (try with -legacy first, fall back without)
    for (p12, pem) in [
        (primary_p12.as_os_str(), pem_a.as_os_str()),
        (std::ffi::OsStr::new(secondary_p12), pem_b.as_os_str()),
    ] {
        let ok = Command::new("openssl")
            .args(["pkcs12", "-in"])
            .arg(p12)
            .args(["-out"])
            .arg(pem)
            .args(["-nodes", "-password", &pass, "-legacy"])
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            Command::new("openssl")
                .args(["pkcs12", "-in"])
                .arg(p12)
                .args(["-out"])
                .arg(pem)
                .args(["-nodes", "-password", &pass])
                .stderr(std::process::Stdio::null())
                .status()?;
        }
    }

    // Concatenate PEM files
    let a = std::fs::read_to_string(&pem_a).unwrap_or_default();
    let b = std::fs::read_to_string(&pem_b).unwrap_or_default();
    std::fs::write(&combined, format!("{a}\n{b}"))?;

    // Re-package into .p12
    let ok = Command::new("openssl")
        .args(["pkcs12", "-export", "-in"])
        .arg(&combined)
        .args(["-out"])
        .arg(primary_p12)
        .args(["-password", &pass, "-legacy"])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        Command::new("openssl")
            .args(["pkcs12", "-export", "-in"])
            .arg(&combined)
            .args(["-out"])
            .arg(primary_p12)
            .args(["-password", &pass])
            .stderr(std::process::Stdio::null())
            .status()?;
    }

    // Clean up
    let _ = std::fs::remove_file(&pem_a);
    let _ = std::fs::remove_file(&pem_b);
    let _ = std::fs::remove_file(&combined);
    Ok(())
}




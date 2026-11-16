//! Entry-file resolution + target / device selection for `perry run`.

use super::*;



/// Resolve the entry TypeScript file
pub fn resolve_entry_file(input: Option<&Path>) -> Result<PathBuf> {
    let project_dir = match input {
        Some(path) if !path.exists() => {
            return Err(anyhow!("File not found: {}", path.display()));
        }
        Some(path) if !path.is_dir() => return Ok(path.to_path_buf()),
        Some(path) => path,
        None => Path::new("."),
    };

    // Try perry.toml
    if let Some(entry) = read_perry_toml_entry(project_dir) {
        if entry.exists() {
            return Ok(entry);
        }
    }

    // Fallback: src/main.ts, then main.ts
    for candidate in &["src/main.ts", "main.ts"] {
        let path = project_dir.join(candidate);
        if path.exists() {
            return Ok(path);
        }
    }

    Err(anyhow!(
        "No input file specified and no main.ts found.\n\
         Usage: perry run <file.ts>\n\
         Or create src/main.ts or main.ts, or set entry in perry.toml"
    ))
}

/// Read entry point from perry.toml if present
fn read_perry_toml_entry(project_dir: &Path) -> Option<PathBuf> {
    let toml_str = std::fs::read_to_string(project_dir.join("perry.toml")).ok()?;
    for line in toml_str.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("entry") {
            if let Some(eq_pos) = trimmed.find('=') {
                let value = trimmed[eq_pos + 1..].trim().trim_matches('"');
                return Some(project_dir.join(value));
            }
        }
    }
    None
}

/// Resolve the compilation target and optional device UDID.
///
/// This fork builds and runs for Linux only, so the only platform accepted is
/// `linux` (or none), and there is never a device.
pub fn resolve_target(
    platform: Option<Platform>,
    _args: &RunArgs,
) -> Result<(Option<String>, Option<String>)> {
    match platform {
        None | Some(Platform::Linux) => Ok((None, None)),
        Some(other) => bail!(
            "this fork of perry builds and runs for Linux only, not {}",
            format!("{other:?}").to_lowercase()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_entry_file};


    #[test]
    fn directory_input_resolves_default_entry() {
        let project = tempfile::tempdir().unwrap();
        let entry = project.path().join("src/main.ts");
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::fs::write(&entry, "console.log('hello');").unwrap();

        assert_eq!(resolve_entry_file(Some(project.path())).unwrap(), entry);
    }

    #[test]
    fn directory_input_resolves_perry_toml_entry() {
        let project = tempfile::tempdir().unwrap();
        let entry = project.path().join("app/index.ts");
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::fs::write(&entry, "console.log('hello');").unwrap();
        std::fs::write(
            project.path().join("perry.toml"),
            "entry = \"app/index.ts\"\n",
        )
        .unwrap();

        assert_eq!(resolve_entry_file(Some(project.path())).unwrap(), entry);
    }

}

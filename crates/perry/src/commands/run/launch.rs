//! Per-target launch dispatch (native, iOS sim/device, Android, web).

use super::*;

/// Launch the compiled output based on target
pub fn launch(
    result: &CompileResult,
    device_udid: Option<&str>,
    program_args: &[String],
    format: OutputFormat,
) -> Result<()> {
    match result.target.as_str() {
        target @ ("android" | "wearos") => {
            bail!("this fork builds and runs for Linux only: it has no {target} packaging")
        }
        _ => launch_native(&result.output_path, program_args, format),
    }
}

/// Launch a native executable
pub fn launch_native(exe_path: &Path, program_args: &[String], format: OutputFormat) -> Result<()> {
    let exe = if exe_path.is_absolute() {
        exe_path.to_path_buf()
    } else {
        std::env::current_dir()?.join(exe_path)
    };

    if !exe.exists() {
        return Err(anyhow!("Compiled executable not found: {}", exe.display()));
    }

    if let OutputFormat::Text = format {
        println!();
        println!("Running {}...", exe_path.display());
        println!();
    }

    // Execute inside the bundle so Foundation and AppKit see its application
    // identity. Direct execution preserves argv, terminal I/O, and exit status.
    let executable = native_executable_path(&exe)?;
    let status = Command::new(&executable)
        .args(program_args)
        .status()
        .map_err(|e| anyhow!("Failed to launch {}: {}", exe.display(), e))?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

fn native_executable_path(output: &Path) -> Result<PathBuf> {
    return Ok(output.to_path_buf());
}





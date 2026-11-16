//! Link every listed function section, including Rust archive members, into
//! one hot prefix. Without an order this adds no arguments or input files.
use anyhow::Result;
use std::{fs, path::Path, process::Command};

pub(super) fn add_link_args(
    cmd: &mut Command,
    cache: &Path,
    linux: bool,
    android: bool,
    harmony: bool,
    direct_apple: bool,
    xlinker_apple: bool,
    runtime_archives: &[&Path],
    lld: bool,
    data_order: bool,
    verbose: u8,
) -> Result<()> {
    let dir = cache.join("function-order");
    let elf = true;
    let order = match perry_codegen::program_function_layout() {
        perry_codegen::FunctionLayout::Order(order) if !order.is_empty() => order,
        _ => {
            // Every JS body has its own input section (so the link can drop
            // each one nothing references); gather them into the one JS text
            // output section whose bounds the stack capture reads.
            {
                // The runtime's default hot-text and hot-data orders, with
                // the same JS text rule; without them, the rule alone as
                // before. The `.rodata` half needs the x86-64 GNU ld script's
                // page alignment in front of `.rodata` (`hot_data_order`).
                if let Some(script) = super::hot_text_order::link_script(
                    cache,
                    runtime_archives,
                    lld,
                    data_order,
                    cfg!(target_arch = "x86_64"),
                    verbose,
                ) {
                    cmd.arg(format!("-Wl,-T,{}", script.display()));
                    return Ok(());
                }
                fs::create_dir_all(&dir)?;
                let rule = perry_codegen::function_order::JS_TEXT_LINK_RULE;
                let script = dir.join("js-text.v1.ld");
                if fs::read_to_string(&script).ok().as_deref() != Some(rule) {
                    write_atomically(&script, rule)?;
                }
                cmd.arg(format!("-Wl,-T,{}", script.display()));
            }
            return Ok(());
        }
    };
    fs::create_dir_all(&dir)?;
    {
        let script = dir.join(format!("{:016x}.v5.ld", order.digest()));
        fs::write(
            &script,
            order.elf_linker_script().map_err(anyhow::Error::msg)?,
        )?;
        cmd.arg(format!("-Wl,-T,{}", script.display()));
        // Preserve ordinary ELF geometry. The native mapper selects complete
        // aligned pages inside the actual selected span after PIE relocation.
        cmd.arg("-Wl,-z,common-page-size=0x1000");
        cmd.arg("-Wl,-z,separate-code");
    }
    Ok(())
}

/// Concurrent links share the cache: a reader never sees a half-written
/// script (write a sibling, then rename over the final name).
fn write_atomically(path: &Path, contents: &str) -> Result<()> {
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    fs::write(&tmp, contents)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

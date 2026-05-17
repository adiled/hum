//! CLI front for the codegen library. Manual regen:
//!
//!   cargo run -p codegen           # write clients/ts/chi.ts
//!   cargo run -p codegen -- --check # exit nonzero if out of date
//!
//! The same logic runs from `thrum-core/build.rs` on every cargo build,
//! so manual invocation should rarely be needed.

use std::process::ExitCode;

use anyhow::{Context, Result};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("codegen: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let mut check = false;
    let mut positional: Vec<String> = Vec::new();
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--check" | "-c" => check = true,
            _ => positional.push(a),
        }
    }
    let target = positional.first().cloned().unwrap_or_else(|| "ts".into());
    let output_override = positional.get(1).map(std::path::PathBuf::from);

    let spec = codegen::parse(&codegen::default_chi_rs(), &codegen::default_lib_rs())
        .context("parse chi spec")?;

    match target.as_str() {
        "ts" => {
            let chi_out = output_override.clone().unwrap_or_else(codegen::default_ts_out);
            let helpers_out = codegen::default_helpers_out();
            if check {
                check_against(&chi_out, &|p| codegen::emit_ts(&spec, p))?;
                check_against(&helpers_out, &codegen::emit_helpers)?;
                eprintln!("codegen: {} + {} up to date", chi_out.display(), helpers_out.display());
            } else {
                codegen::emit_ts(&spec, &chi_out)?;
                codegen::emit_helpers(&helpers_out)?;
                eprintln!("codegen ts: {} ({} chi, {} pulse) -> {} + {}",
                    spec.version, spec.chi.len(), spec.pulse.len(),
                    chi_out.display(), helpers_out.display());
            }
        }
        other => anyhow::bail!("unknown target {other}; valid: ts"),
    }
    Ok(())
}

fn check_against<F>(output: &std::path::Path, emit: &F) -> Result<()>
where
    F: Fn(&std::path::Path) -> Result<()>,
{
    let tmp = tempfile_path(output);
    emit(&tmp)?;
    let generated = std::fs::read(&tmp).context("read tmp")?;
    let _ = std::fs::remove_file(&tmp);
    let current = std::fs::read(output).unwrap_or_default();
    if current != generated {
        anyhow::bail!(
            "{} is out of date; run `cargo run -p codegen` to regenerate",
            output.display()
        );
    }
    Ok(())
}

fn tempfile_path(target: &std::path::Path) -> std::path::PathBuf {
    let mut p = target.to_path_buf();
    let name = format!(
        ".{}.codegen-check",
        target.file_name().and_then(|f| f.to_str()).unwrap_or("out")
    );
    p.set_file_name(name);
    p
}

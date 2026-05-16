//! CLI front for the codegen library. Manual regen:
//!
//!   cargo run -p codegen           # write thrum/chi.ts
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
            let output = output_override.unwrap_or_else(codegen::default_ts_out);
            if check {
                let mut buf = Vec::new();
                let tmp = tempfile_path(&output);
                codegen::emit_ts(&spec, &tmp)?;
                let generated = std::fs::read(&tmp).context("read tmp")?;
                let _ = std::fs::remove_file(&tmp);
                buf.extend_from_slice(&generated);
                let current = std::fs::read(&output).unwrap_or_default();
                if current != buf {
                    anyhow::bail!(
                        "{} is out of date; run `cargo run -p codegen` to regenerate",
                        output.display()
                    );
                }
                eprintln!("codegen: {} up to date ({} chi, {} pulse)", output.display(), spec.chi.len(), spec.pulse.len());
            } else {
                codegen::emit_ts(&spec, &output)?;
                eprintln!("codegen ts: {} ({} chi, {} pulse) -> {}", spec.version, spec.chi.len(), spec.pulse.len(), output.display());
            }
        }
        other => anyhow::bail!("unknown target {other}; valid: ts"),
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

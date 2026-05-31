//! CLI front for the codegen library. Manual regen:
//!
//!   cargo run -p codegen                # regen all targets (ts, python, go)
//!   cargo run -p codegen -- ts          # one target
//!   cargo run -p codegen -- --check     # exit nonzero if any target drifted
//!
//! The same logic runs from `thrum-core/build.rs` on every cargo build,
//! so manual invocation should rarely be needed. Output locations live
//! in `codegen::paths` — never hand-write them here.

use std::process::ExitCode;

use anyhow::{Context, Result};
use codegen::{paths, ChiSpec};

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
    let targets: Vec<String> = if positional.is_empty() {
        vec!["ts".into(), "python".into(), "go".into()]
    } else {
        positional
    };

    let spec = codegen::parse(&paths::chi_rs(), &paths::lib_rs())
        .context("parse chi spec")?;

    for t in &targets {
        run_target(t, &spec, check)?;
    }
    Ok(())
}

fn run_target(target: &str, spec: &ChiSpec, check: bool) -> Result<()> {
    let (chi_out, helpers_out, emit_chi, emit_helpers): (
        std::path::PathBuf,
        std::path::PathBuf,
        Box<dyn Fn(&std::path::Path) -> Result<()>>,
        Box<dyn Fn(&std::path::Path) -> Result<()>>,
    ) = match target {
        "ts" => (
            paths::ts_chi(),
            paths::ts_helpers(),
            Box::new(|p| codegen::emit_ts(spec, p)),
            Box::new(codegen::emit_helpers),
        ),
        "python" | "py" => (
            paths::py_chi(),
            paths::py_helpers(),
            Box::new(|p| codegen::emit_py(spec, p)),
            Box::new(codegen::emit_py_helpers),
        ),
        "go" => (
            paths::go_chi(),
            paths::go_helpers(),
            Box::new(|p| codegen::emit_go(spec, p)),
            Box::new(codegen::emit_go_helpers),
        ),
        other => anyhow::bail!("unknown target {other}; valid: ts, python, go"),
    };

    if check {
        check_against(&chi_out, &emit_chi)?;
        check_against(&helpers_out, &emit_helpers)?;
        eprintln!("codegen: {} + {} up to date", chi_out.display(), helpers_out.display());
    } else {
        emit_chi(&chi_out)?;
        emit_helpers(&helpers_out)?;
        eprintln!(
            "codegen {target}: {} ({} chi, {} pulse) -> {} + {}",
            spec.version, spec.chi.len(), spec.pulse.len(),
            chi_out.display(), helpers_out.display(),
        );
    }
    Ok(())
}

fn check_against(output: &std::path::Path, emit: &dyn Fn(&std::path::Path) -> Result<()>) -> Result<()> {
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

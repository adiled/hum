//! CLI front for the codegen library. Manual regen:
//!
//!   cargo run -p codegen                # regen all targets (ts, python, go, c)
//!   cargo run -p codegen -- ts          # one target
//!   cargo run -p codegen -- --check     # exit nonzero if any target drifted
//!
//! Every run first holds thrum-core's Rust to `hum.thrum` and stops on
//! a difference: the file is the thrum, and a client emitted while the
//! Rust disagrees would be a client of two thrums. The same logic runs
//! from `thrum-core/build.rs` on every cargo build, so manual invocation
//! should rarely be needed. Output locations live in `codegen::paths`;
//! never hand-write them here.
//!
//! A target is the files it owns: the registry and the views projected
//! from `hum.thrum`, and the client itself, each language's template
//! from `codegen/templates/` with the link's constants read from
//! hum-thrum. Every source file of every client is generated. After the
//! targets, WIRE.md's marked regions are filled from the same file.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use codegen::{paths, ChiSpec};

type Emit<'a> = Box<dyn Fn(&Path) -> Result<()> + 'a>;

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
        vec!["ts".into(), "python".into(), "go".into(), "c".into()]
    } else {
        positional
    };

    let thrum = codegen::thrum::parse(&paths::hum_thrum()).context("read the thrum")?;
    codegen::rust::check(
        &thrum,
        &paths::chi_rs(),
        &paths::views_rs(),
        &paths::envelope_rs(),
        &paths::lib_rs(),
    )?;
    eprintln!("codegen: thrum-core agrees with {}", paths::HUM_THRUM_REL);
    let spec = thrum.chi_spec()?;
    let proto = thrum.protocol_spec();
    let link = codegen::link::link_spec(&paths::link_rs(), &paths::core_rs(), &thrum)
        .context("read the link")?;

    for t in &targets {
        run_target(t, &spec, &proto, &link, check)?;
    }
    run_wire(&thrum, check)
}

fn run_wire(thrum: &codegen::thrum::Thrum, check: bool) -> Result<()> {
    let out = paths::wire_md();
    let emit: Emit = Box::new(move |p| codegen::wire::emit(thrum, &paths::wire_md(), p));
    if check {
        check_against(&out, &emit)?;
        eprintln!("codegen: {} up to date", out.display());
    } else {
        emit(&out)?;
        eprintln!(
            "codegen wire: {} regions of {} filled from {}",
            codegen::wire::REGIONS.len(),
            out.display(),
            paths::HUM_THRUM_REL
        );
    }
    Ok(())
}

fn outputs<'a>(
    target: &str,
    spec: &'a ChiSpec,
    proto: &'a codegen::protocol::ProtocolSpec,
    link: &'a codegen::link::LinkSpec,
) -> Result<Vec<(PathBuf, Emit<'a>)>> {
    let target = if target == "py" { "python" } else { target };
    let mut out = registry(target, spec, proto)?;
    for client in paths::CLIENTS.iter().filter(|c| c.target == target) {
        out.push((
            client.output_path(),
            Box::new(move |p| codegen::link::emit_client(link, client, p)),
        ));
    }
    Ok(out)
}

fn registry<'a>(
    target: &str,
    spec: &'a ChiSpec,
    proto: &'a codegen::protocol::ProtocolSpec,
) -> Result<Vec<(PathBuf, Emit<'a>)>> {
    Ok(match target {
        "ts" => vec![
            (paths::ts_chi(), Box::new(|p| codegen::emit_ts(spec, p))),
            (
                paths::ts_protocol(),
                Box::new(|p| codegen::protocol::emit_protocol_ts(proto, p)),
            ),
        ],
        "python" => vec![
            (paths::py_chi(), Box::new(|p| codegen::emit_py(spec, p))),
            (
                paths::py_protocol(),
                Box::new(|p| codegen::protocol::emit_protocol_py(proto, p)),
            ),
        ],
        "go" => vec![
            (paths::go_chi(), Box::new(|p| codegen::emit_go(spec, p))),
            (
                paths::go_protocol(),
                Box::new(|p| codegen::protocol::emit_protocol_go(proto, p)),
            ),
        ],
        "c" => vec![
            (paths::c_chi(), Box::new(|p| codegen::emit_c(spec, p))),
            (
                paths::c_protocol(),
                Box::new(|p| codegen::protocol::emit_protocol_c(proto, p)),
            ),
        ],
        other => anyhow::bail!("unknown target {other}; valid: ts, python, go, c"),
    })
}

fn run_target(
    target: &str,
    spec: &ChiSpec,
    proto: &codegen::protocol::ProtocolSpec,
    link: &codegen::link::LinkSpec,
    check: bool,
) -> Result<()> {
    let outputs = outputs(target, spec, proto, link)?;
    let names = outputs
        .iter()
        .map(|(out, _)| out.display().to_string())
        .collect::<Vec<_>>()
        .join(" + ");

    if check {
        for (out, emit) in &outputs {
            check_against(out, emit)?;
        }
        eprintln!("codegen: {names} up to date");
    } else {
        for (out, emit) in &outputs {
            emit(out)?;
        }
        eprintln!(
            "codegen {target}: {} ({} chi, {} pulse, {} petal) -> {names}",
            spec.version,
            spec.chi.len(),
            spec.pulse.len(),
            spec.petal.len(),
        );
    }
    Ok(())
}

fn check_against(output: &Path, emit: &dyn Fn(&Path) -> Result<()>) -> Result<()> {
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

fn tempfile_path(target: &Path) -> PathBuf {
    let mut p = target.to_path_buf();
    let name = format!(
        ".{}.codegen-check",
        target.file_name().and_then(|f| f.to_str()).unwrap_or("out")
    );
    p.set_file_name(name);
    p
}

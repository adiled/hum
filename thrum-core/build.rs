//! Build script: keeps the generated clients in `thrum-clients/{ts,python,go,c}/`
//! in lockstep with the wire contract and the link.
//!
//! Cargo reruns this whenever the listed files change. We parse the
//! Rust enums and `WIRE.md` via the `codegen` library and regenerate
//! every target. No drift possible: every build of thrum-core refreshes
//! every client. Output paths live in `codegen::paths` — never
//! hand-write them here.

use codegen::paths;

fn main() {
    let chi_rs = paths::chi_rs();
    let lib_rs = paths::lib_rs();
    let wire_md = paths::wire_md();
    let views_rs = paths::views_rs();
    let envelope_rs = paths::envelope_rs();

    println!("cargo:rerun-if-changed={}", chi_rs.display());
    println!("cargo:rerun-if-changed={}", lib_rs.display());
    println!("cargo:rerun-if-changed={}", wire_md.display());
    println!("cargo:rerun-if-changed={}", views_rs.display());
    println!("cargo:rerun-if-changed={}", envelope_rs.display());
    println!("cargo:rerun-if-changed={}", paths::link_rs().display());
    println!("cargo:rerun-if-changed={}", paths::core_rs().display());
    println!("cargo:rerun-if-changed={}", paths::templates().display());
    println!("cargo:rerun-if-changed=build.rs");

    let spec = match codegen::parse(&chi_rs, &lib_rs, &wire_md) {
        Ok(s) => s,
        Err(e) => {
            println!("cargo:warning=thrum-core build.rs: parse failed: {e}");
            return;
        }
    };
    let proto = match codegen::protocol::protocol_spec(&views_rs, &envelope_rs, &lib_rs) {
        Ok(p) => p,
        Err(e) => {
            println!("cargo:warning=thrum-core build.rs: protocol parse failed: {e}");
            return;
        }
    };
    if let Err(e) = codegen::protocol::check_coverage(&spec.chi, &proto) {
        println!("cargo:warning=thrum-core build.rs: coverage: {e}");
        return;
    }

    let link = match codegen::link::link_spec(
        &paths::link_rs(),
        &paths::core_rs(),
        &chi_rs,
        &lib_rs,
        &wire_md,
    ) {
        Ok(l) => l,
        Err(e) => {
            println!("cargo:warning=thrum-core build.rs: link parse failed: {e}");
            return;
        }
    };

    let mut emits: Vec<(String, std::path::PathBuf, Box<dyn Fn() -> codegen::Result<()> + '_>)> = vec![
        ("emit_ts".into(), paths::ts_chi(), Box::new(|| codegen::emit_ts(&spec, &paths::ts_chi()))),
        ("emit_protocol_ts".into(), paths::ts_protocol(), Box::new(|| {
            codegen::protocol::emit_protocol_ts(&proto, &paths::ts_protocol())
        })),
        ("emit_py".into(), paths::py_chi(), Box::new(|| codegen::emit_py(&spec, &paths::py_chi()))),
        ("emit_protocol_py".into(), paths::py_protocol(), Box::new(|| {
            codegen::protocol::emit_protocol_py(&proto, &paths::py_protocol())
        })),
        ("emit_go".into(), paths::go_chi(), Box::new(|| codegen::emit_go(&spec, &paths::go_chi()))),
        ("emit_protocol_go".into(), paths::go_protocol(), Box::new(|| {
            codegen::protocol::emit_protocol_go(&proto, &paths::go_protocol())
        })),
        ("emit_c".into(), paths::c_chi(), Box::new(|| codegen::emit_c(&spec, &paths::c_chi()))),
        ("emit_protocol_c".into(), paths::c_protocol(), Box::new(|| {
            codegen::protocol::emit_protocol_c(&proto, &paths::c_protocol())
        })),
    ];
    for client in paths::CLIENTS {
        let link = &link;
        emits.push((
            format!("emit_client {}", client.output),
            client.output_path(),
            Box::new(move || codegen::link::emit_client(link, client, &client.output_path())),
        ));
    }
    for (label, _out, emit) in &emits {
        if let Err(e) = emit() {
            println!("cargo:warning=thrum-core build.rs: {label}: {e}");
        }
    }
}

//! Build script: keeps the generated clients in `thrum-clients/{ts,python,go,c}/`
//! and WIRE.md's generated regions in lockstep with `hum.thrum`, and says
//! when this crate's own Rust is not.
//!
//! Cargo reruns this whenever the listed files change. The `codegen`
//! library reads the thrum, compares `chi.rs`, `views.rs`, `envelope.rs`
//! and `lib.rs` with it (a difference is a cargo warning here, and a
//! failure of `cargo run -p codegen -- --check` in CI), regenerates
//! every target, and fills WIRE.md. Output paths live in
//! `codegen::paths`; never hand-write them here.

use codegen::paths;

fn main() {
    let hum_thrum = paths::hum_thrum();
    let chi_rs = paths::chi_rs();
    let lib_rs = paths::lib_rs();
    let views_rs = paths::views_rs();
    let envelope_rs = paths::envelope_rs();

    println!("cargo:rerun-if-changed={}", hum_thrum.display());
    println!("cargo:rerun-if-changed={}", paths::wire_md().display());
    println!("cargo:rerun-if-changed={}", chi_rs.display());
    println!("cargo:rerun-if-changed={}", lib_rs.display());
    println!("cargo:rerun-if-changed={}", views_rs.display());
    println!("cargo:rerun-if-changed={}", envelope_rs.display());
    println!("cargo:rerun-if-changed={}", paths::link_rs().display());
    println!("cargo:rerun-if-changed={}", paths::core_rs().display());
    println!("cargo:rerun-if-changed={}", paths::templates().display());
    println!("cargo:rerun-if-changed=build.rs");

    let thrum = match codegen::thrum::parse(&hum_thrum) {
        Ok(t) => t,
        Err(e) => {
            warn(&format!("{e:#}"));
            return;
        }
    };
    if let Err(e) = codegen::rust::check(&thrum, &chi_rs, &views_rs, &envelope_rs, &lib_rs) {
        warn(&format!("{e:#}"));
    }
    let spec = match thrum.chi_spec() {
        Ok(s) => s,
        Err(e) => {
            warn(&format!("{e:#}"));
            return;
        }
    };
    let proto = thrum.protocol_spec();
    let link = match codegen::link::link_spec(&paths::link_rs(), &paths::core_rs(), &thrum) {
        Ok(l) => l,
        Err(e) => {
            warn(&format!("link: {e:#}"));
            return;
        }
    };

    let mut emits: Vec<(String, Box<dyn Fn() -> codegen::Result<()> + '_>)> = vec![
        ("emit_ts".into(), Box::new(|| codegen::emit_ts(&spec, &paths::ts_chi()))),
        ("emit_protocol_ts".into(), Box::new(|| {
            codegen::protocol::emit_protocol_ts(&proto, &paths::ts_protocol())
        })),
        ("emit_py".into(), Box::new(|| codegen::emit_py(&spec, &paths::py_chi()))),
        ("emit_protocol_py".into(), Box::new(|| {
            codegen::protocol::emit_protocol_py(&proto, &paths::py_protocol())
        })),
        ("emit_go".into(), Box::new(|| codegen::emit_go(&spec, &paths::go_chi()))),
        ("emit_protocol_go".into(), Box::new(|| {
            codegen::protocol::emit_protocol_go(&proto, &paths::go_protocol())
        })),
        ("emit_c".into(), Box::new(|| codegen::emit_c(&spec, &paths::c_chi()))),
        ("emit_protocol_c".into(), Box::new(|| {
            codegen::protocol::emit_protocol_c(&proto, &paths::c_protocol())
        })),
    ];
    for client in paths::CLIENTS {
        let link = &link;
        emits.push((
            format!("emit_client {}", client.output),
            Box::new(move || codegen::link::emit_client(link, client, &client.output_path())),
        ));
    }
    emits.push((
        format!("fill {}", paths::WIRE_MD_REL),
        Box::new(|| codegen::wire::emit(&thrum, &paths::wire_md(), &paths::wire_md())),
    ));
    for (label, emit) in &emits {
        if let Err(e) = emit() {
            warn(&format!("{label}: {e:#}"));
        }
    }
}

fn warn(message: &str) {
    for line in message.lines() {
        println!("cargo:warning=thrum-core build.rs: {line}");
    }
}

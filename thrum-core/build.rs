//! Build script — keeps generated SDKs in `thrum-clients/{ts,python,go}/`
//! in lockstep with `chi.rs`.
//!
//! Cargo reruns this whenever the listed files change. We parse the
//! Rust enums via the `codegen` library and regenerate every target.
//! No drift possible: every build of thrum-core refreshes every client.
//! Output paths live in `codegen::paths` — never hand-write them here.

use codegen::paths;

fn main() {
    let chi_rs = paths::chi_rs();
    let lib_rs = paths::lib_rs();
    let views_rs = paths::views_rs();
    let envelope_rs = paths::envelope_rs();

    println!("cargo:rerun-if-changed={}", chi_rs.display());
    println!("cargo:rerun-if-changed={}", lib_rs.display());
    println!("cargo:rerun-if-changed={}", views_rs.display());
    println!("cargo:rerun-if-changed={}", envelope_rs.display());
    println!("cargo:rerun-if-changed=build.rs");

    let spec = match codegen::parse(&chi_rs, &lib_rs) {
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

    let emits: [(&str, std::path::PathBuf, &dyn Fn() -> codegen::Result<()>); 9] = [
        ("emit_ts", paths::ts_chi(), &|| {
            codegen::emit_ts(&spec, &paths::ts_chi())
        }),
        ("emit_helpers", paths::ts_helpers(), &|| {
            codegen::emit_helpers(&paths::ts_helpers())
        }),
        ("emit_protocol_ts", paths::ts_protocol(), &|| {
            codegen::protocol::emit_protocol_ts(&proto, &paths::ts_protocol())
        }),
        ("emit_py", paths::py_chi(), &|| {
            codegen::emit_py(&spec, &paths::py_chi())
        }),
        ("emit_py_helpers", paths::py_helpers(), &|| {
            codegen::emit_py_helpers(&paths::py_helpers())
        }),
        ("emit_protocol_py", paths::py_protocol(), &|| {
            codegen::protocol::emit_protocol_py(&proto, &paths::py_protocol())
        }),
        ("emit_go", paths::go_chi(), &|| {
            codegen::emit_go(&spec, &paths::go_chi())
        }),
        ("emit_go_helpers", paths::go_helpers(), &|| {
            codegen::emit_go_helpers(&paths::go_helpers())
        }),
        ("emit_protocol_go", paths::go_protocol(), &|| {
            codegen::protocol::emit_protocol_go(&proto, &paths::go_protocol())
        }),
    ];
    for (label, _out, emit) in &emits {
        if let Err(e) = emit() {
            println!("cargo:warning=thrum-core build.rs: {label}: {e}");
        }
    }
}

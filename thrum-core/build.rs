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

    println!("cargo:rerun-if-changed={}", chi_rs.display());
    println!("cargo:rerun-if-changed={}", lib_rs.display());
    println!("cargo:rerun-if-changed=build.rs");

    let spec = match codegen::parse(&chi_rs, &lib_rs) {
        Ok(s) => s,
        Err(e) => {
            println!("cargo:warning=thrum-core build.rs: parse failed: {e}");
            return;
        }
    };

    let emits: [(&str, std::path::PathBuf, &dyn Fn(&std::path::Path) -> codegen::Result<()>); 6] = [
        ("emit_ts",         paths::ts_chi(),     &|p| codegen::emit_ts(&spec, p)),
        ("emit_helpers",    paths::ts_helpers(), &codegen::emit_helpers),
        ("emit_py",         paths::py_chi(),     &|p| codegen::emit_py(&spec, p)),
        ("emit_py_helpers", paths::py_helpers(), &codegen::emit_py_helpers),
        ("emit_go",         paths::go_chi(),     &|p| codegen::emit_go(&spec, p)),
        ("emit_go_helpers", paths::go_helpers(), &codegen::emit_go_helpers),
    ];
    for (label, out, emit) in &emits {
        if let Err(e) = emit(out) {
            println!("cargo:warning=thrum-core build.rs: {label}: {e}");
        }
    }
}

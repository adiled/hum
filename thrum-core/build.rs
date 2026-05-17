//! Build script — keeps `thrum-clients/ts/chi.ts` in lockstep with `chi.rs`.
//!
//! Cargo reruns this whenever the listed files change. We parse the
//! Rust enums via the `codegen` library and regenerate the TS file.
//! No drift possible: every build of thrum-core refreshes the TS side.

use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let chi_rs = manifest.join("src/chi.rs");
    let lib_rs = manifest.join("src/lib.rs");
    let ts_out = manifest.join("../thrum-clients/ts/chi.ts");

    // Cargo: rerun only when these change. Without these directives the
    // build script reruns every build (cheap, but noisier output).
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

    if let Err(e) = codegen::emit_ts(&spec, &ts_out) {
        println!("cargo:warning=thrum-core build.rs: emit_ts failed: {e}");
    }

    let helpers_out = manifest.join("../thrum-clients/ts/helpers.ts");
    if let Err(e) = codegen::emit_helpers(&helpers_out) {
        println!("cargo:warning=thrum-core build.rs: emit_helpers failed: {e}");
    }

    // Python target.
    let py_out = manifest.join("../thrum-clients/python/thrum/chi.py");
    if let Err(e) = codegen::emit_py(&spec, &py_out) {
        println!("cargo:warning=thrum-core build.rs: emit_py failed: {e}");
    }
    let py_helpers_out = manifest.join("../thrum-clients/python/thrum/helpers.py");
    if let Err(e) = codegen::emit_py_helpers(&py_helpers_out) {
        println!("cargo:warning=thrum-core build.rs: emit_py_helpers failed: {e}");
    }

    // Go target.
    let go_out = manifest.join("../thrum-clients/go/thrum/chi.go");
    if let Err(e) = codegen::emit_go(&spec, &go_out) {
        println!("cargo:warning=thrum-core build.rs: emit_go failed: {e}");
    }
    let go_helpers_out = manifest.join("../thrum-clients/go/thrum/helpers.go");
    if let Err(e) = codegen::emit_go_helpers(&go_helpers_out) {
        println!("cargo:warning=thrum-core build.rs: emit_go_helpers failed: {e}");
    }
}

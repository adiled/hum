//! Source-tree paths owned by codegen.
//!
//! Every literal path under `thrum-core/src/...` and `thrum-clients/...`
//! lives in this file. Callers (build.rs, the CLI) MUST go through
//! these functions — never hand-write the layout, or the registry will
//! drift the moment somebody renames a directory (see commit history).
//!
//! Anchored to this crate's `CARGO_MANIFEST_DIR`. `codegen/` and the
//! source/output trees are workspace siblings, so `<codegen>/..` is
//! the repo root.

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

// ── inputs (thrum-core source of truth) ───────────────────────────────────

pub fn chi_rs() -> PathBuf {
    repo_root().join("thrum-core/src/chi.rs")
}

pub fn lib_rs() -> PathBuf {
    repo_root().join("thrum-core/src/lib.rs")
}

// ── outputs (generated client SDKs) ───────────────────────────────────────

pub fn ts_chi() -> PathBuf {
    repo_root().join("thrum-clients/ts/chi.ts")
}

pub fn ts_helpers() -> PathBuf {
    repo_root().join("thrum-clients/ts/helpers.ts")
}

pub fn py_chi() -> PathBuf {
    repo_root().join("thrum-clients/python/thrum/chi.py")
}

pub fn py_helpers() -> PathBuf {
    repo_root().join("thrum-clients/python/thrum/helpers.py")
}

pub fn go_chi() -> PathBuf {
    repo_root().join("thrum-clients/go/thrum/chi.go")
}

pub fn go_helpers() -> PathBuf {
    repo_root().join("thrum-clients/go/thrum/helpers.go")
}

//! Source-tree paths owned by codegen.
//!
//! Every literal path under `thrum-core/src/...` and `thrum-clients/...`
//! lives in this file. Callers (build.rs, the CLI, the emitters in
//! lib.rs) MUST go through these constants and functions, never
//! hand-write the layout. Otherwise the registry will drift the moment
//! somebody renames a directory.
//!
//! Anchored to this crate's `CARGO_MANIFEST_DIR`. `codegen/` and the
//! source/output trees are workspace siblings, so `<codegen>/..` is
//! the repo root.
//!
//! The `*_REL` constants are the repo-relative strings. Generated-file
//! headers reference them by name, so the @generated comments stay
//! truthful through renames. The `*()` functions resolve them to
//! absolute paths for disk I/O.

use std::path::PathBuf;

// ── repo-relative strings (single source of truth) ────────────────────────

pub const CHI_RS_REL: &str = "thrum-core/src/chi.rs";
pub const LIB_RS_REL: &str = "thrum-core/src/lib.rs";
/// Wire-view sources — parsed into typed protocol mirrors for each client.
pub const VIEWS_RS_REL: &str = "thrum-core/src/views.rs";
pub const ENVELOPE_RS_REL: &str = "thrum-core/src/envelope.rs";
/// Compact ref for the two runtime-helper sources, used in the
/// "Runtime helpers that mirror …" header emitted into every helpers
/// file. They live next to chi.rs / lib.rs in `thrum-core/src/`.
pub const HELPERS_SOURCE_REF: &str = "thrum-core/src/{prims,wane}.rs";

pub const TS_CHI_REL: &str = "thrum-clients/ts/chi.ts";
pub const TS_HELPERS_REL: &str = "thrum-clients/ts/helpers.ts";
pub const TS_PROTOCOL_REL: &str = "thrum-clients/ts/protocol.ts";
pub const PY_CHI_REL: &str = "thrum-clients/python/thrum/chi.py";
pub const PY_HELPERS_REL: &str = "thrum-clients/python/thrum/helpers.py";
pub const PY_PROTOCOL_REL: &str = "thrum-clients/python/thrum/protocol.py";
pub const GO_CHI_REL: &str = "thrum-clients/go/thrum/chi.go";
pub const GO_HELPERS_REL: &str = "thrum-clients/go/thrum/helpers.go";
pub const GO_PROTOCOL_REL: &str = "thrum-clients/go/thrum/protocol.go";

// ── absolute resolution ───────────────────────────────────────────────────

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

pub fn chi_rs() -> PathBuf {
    repo_root().join(CHI_RS_REL)
}
pub fn lib_rs() -> PathBuf {
    repo_root().join(LIB_RS_REL)
}
pub fn views_rs() -> PathBuf {
    repo_root().join(VIEWS_RS_REL)
}
pub fn envelope_rs() -> PathBuf {
    repo_root().join(ENVELOPE_RS_REL)
}
pub fn ts_chi() -> PathBuf {
    repo_root().join(TS_CHI_REL)
}
pub fn ts_helpers() -> PathBuf {
    repo_root().join(TS_HELPERS_REL)
}
pub fn ts_protocol() -> PathBuf {
    repo_root().join(TS_PROTOCOL_REL)
}
pub fn py_chi() -> PathBuf {
    repo_root().join(PY_CHI_REL)
}
pub fn py_helpers() -> PathBuf {
    repo_root().join(PY_HELPERS_REL)
}
pub fn py_protocol() -> PathBuf {
    repo_root().join(PY_PROTOCOL_REL)
}
pub fn go_chi() -> PathBuf {
    repo_root().join(GO_CHI_REL)
}
pub fn go_helpers() -> PathBuf {
    repo_root().join(GO_HELPERS_REL)
}
pub fn go_protocol() -> PathBuf {
    repo_root().join(GO_PROTOCOL_REL)
}

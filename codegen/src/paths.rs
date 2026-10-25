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


pub const CHI_RS_REL: &str = "thrum-core/src/chi.rs";
pub const LIB_RS_REL: &str = "thrum-core/src/lib.rs";
pub const WIRE_MD_REL: &str = "WIRE.md";
pub const VIEWS_RS_REL: &str = "thrum-core/src/views.rs";
pub const ENVELOPE_RS_REL: &str = "thrum-core/src/envelope.rs";
pub const LINK_RS_REL: &str = "hum-thrum/src/link.rs";
pub const CORE_RS_REL: &str = "hum-thrum/src/core.rs";
pub const TEMPLATES_REL: &str = "codegen/templates";

pub const TS_CHI_REL: &str = "thrum-clients/ts/chi.ts";
pub const TS_PROTOCOL_REL: &str = "thrum-clients/ts/protocol.ts";
pub const PY_CHI_REL: &str = "thrum-clients/python/thrum/chi.py";
pub const PY_PROTOCOL_REL: &str = "thrum-clients/python/thrum/protocol.py";
pub const GO_CHI_REL: &str = "thrum-clients/go/thrum/chi.go";
pub const GO_PROTOCOL_REL: &str = "thrum-clients/go/thrum/protocol.go";
pub const C_CHI_REL: &str = "thrum-clients/c/thrum/chi.h";
pub const C_PROTOCOL_REL: &str = "thrum-clients/c/thrum/protocol.h";


fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

pub fn chi_rs() -> PathBuf {
    repo_root().join(CHI_RS_REL)
}
pub fn lib_rs() -> PathBuf {
    repo_root().join(LIB_RS_REL)
}
pub fn wire_md() -> PathBuf {
    repo_root().join(WIRE_MD_REL)
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
pub fn ts_protocol() -> PathBuf {
    repo_root().join(TS_PROTOCOL_REL)
}
pub fn py_chi() -> PathBuf {
    repo_root().join(PY_CHI_REL)
}
pub fn py_protocol() -> PathBuf {
    repo_root().join(PY_PROTOCOL_REL)
}
pub fn go_chi() -> PathBuf {
    repo_root().join(GO_CHI_REL)
}
pub fn go_protocol() -> PathBuf {
    repo_root().join(GO_PROTOCOL_REL)
}
pub fn c_chi() -> PathBuf {
    repo_root().join(C_CHI_REL)
}
pub fn c_protocol() -> PathBuf {
    repo_root().join(C_PROTOCOL_REL)
}
pub fn link_rs() -> PathBuf {
    repo_root().join(LINK_RS_REL)
}
pub fn core_rs() -> PathBuf {
    repo_root().join(CORE_RS_REL)
}
pub fn templates() -> PathBuf {
    repo_root().join(TEMPLATES_REL)
}

pub struct Client {
    pub target: &'static str,
    pub template: &'static str,
    pub output: &'static str,
    pub comment: &'static str,
    pub constants: bool,
}

impl Client {
    pub fn template_rel(&self) -> String {
        format!("{TEMPLATES_REL}/{}", self.template)
    }
    pub fn template_path(&self) -> PathBuf {
        templates().join(self.template)
    }
    pub fn output_path(&self) -> PathBuf {
        repo_root().join(self.output)
    }
}

pub const CLIENTS: &[Client] = &[
    Client { target: "ts", template: "ts/link.ts", output: "thrum-clients/ts/link.ts", comment: "//", constants: true },
    Client { target: "ts", template: "ts/index.ts", output: "thrum-clients/ts/index.ts", comment: "//", constants: false },
    Client { target: "python", template: "python/link.py", output: "thrum-clients/python/thrum/link.py", comment: "#", constants: true },
    Client { target: "python", template: "python/__init__.py", output: "thrum-clients/python/thrum/__init__.py", comment: "#", constants: false },
    Client { target: "go", template: "go/link.go", output: "thrum-clients/go/thrum/link.go", comment: "//", constants: true },
    Client { target: "c", template: "c/link.h", output: "thrum-clients/c/thrum/link.h", comment: "//", constants: true },
    Client { target: "c", template: "c/link.c", output: "thrum-clients/c/thrum/link.c", comment: "//", constants: false },
    Client { target: "c", template: "c/thrum.h", output: "thrum-clients/c/thrum/thrum.h", comment: "//", constants: false },
];

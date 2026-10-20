//! hum.json loader (0.3).
//!
//! Namespaced daemon policy: `humd` / `fs` / `nest`. Missing sections
//! fill with defaults. Validity is defined by `hum.schema.json`
//! ([`SCHEMA`]); [`validate_or_exit`] gates daemon startup on it, so an
//! invalid config is fatal rather than silently coerced.
//!
//! A hive's own runtime — binary, models, flags — is owned by the hive
//! process through its service env, not by humd. A `hives` block in
//! hum.json is the wrong place, and is a lint finding rather than a
//! silent no-op.
//!
//! serde tolerates unknown keys, so validity is enforced by the schema
//! rather than by the deserializer.
//!
//! Keys a released humd wrote and a later one no longer reads are
//! [`RETIRED`]. A daemon that dies because the file carries a key it has
//! moved on from turns every upgrade into an outage, so a retired key is
//! dropped on sight instead of failing validation. A key that was never
//! known is still a violation.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::warn;


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumdSection {
    #[serde(default = "defaults::permission_dusk_ms", rename = "permissionDuskMs")]
    pub permission_dusk_ms: u64,
    #[serde(default = "defaults::metrics_addr", rename = "metricsAddr")]
    pub metrics_addr: String,
    #[serde(default, rename = "tcpListen")]
    pub tcp_listen: Option<String>,
    #[serde(default, rename = "trustRemoteWorkers")]
    pub trust_remote_workers: bool,
}

impl Default for HumdSection {
    fn default() -> Self {
        Self {
            permission_dusk_ms: defaults::permission_dusk_ms(),
            metrics_addr: defaults::metrics_addr(),
            tcp_listen: None,
            trust_remote_workers: false,
        }
    }
}


#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FsMode {
    #[default]
    Rw,
    Ro,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsRoot {
    pub path: PathBuf,
    #[serde(default)]
    pub mode: FsMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsSection {
    #[serde(default)]
    pub roots: Vec<FsRoot>,
    #[serde(default = "defaults::denied")]
    pub denied: Vec<PathBuf>,
}

impl Default for FsSection {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            denied: defaults::denied(),
        }
    }
}

impl FsSection {
    pub fn check(&self, candidate: &Path) -> Result<FsMode, FsDenial> {
        let abs = canonical_or_self(candidate);
        for d in &self.denied {
            if starts_with(&abs, d) {
                return Err(FsDenial::Blacklisted(d.clone()));
            }
        }
        for root in &self.roots {
            if starts_with(&abs, &root.path) {
                return Ok(root.mode);
            }
        }
        Err(FsDenial::OutsideRoots)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsDenial {
    Blacklisted(PathBuf),
    OutsideRoots,
}

impl std::fmt::Display for FsDenial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FsDenial::Blacklisted(p) => write!(f, "blacklisted by fs.denied prefix {}", p.display()),
            FsDenial::OutsideRoots => f.write_str("outside fs.roots"),
        }
    }
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NestSection {
    #[serde(default = "defaults::max_active_cells", rename = "maxActiveCells")]
    pub max_active_cells: u32,
    #[serde(default = "defaults::cell_idle_prune_threshold_ms", rename = "cellIdlePruneThresholdMs")]
    pub cell_idle_prune_threshold_ms: u64,
    #[serde(default = "defaults::default_hive")]
    pub default: String,
}

impl Default for NestSection {
    fn default() -> Self {
        Self {
            max_active_cells: defaults::max_active_cells(),
            cell_idle_prune_threshold_ms: defaults::cell_idle_prune_threshold_ms(),
            default: defaults::default_hive(),
        }
    }
}


#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HumConfig {
    #[serde(default)]
    pub humd: HumdSection,
    #[serde(default)]
    pub fs: FsSection,
    #[serde(default)]
    pub nest: NestSection,
}


pub fn config_path() -> PathBuf {
    crate::hum_json()
}

fn expand_tilde(p: &Path) -> PathBuf {
    crate::expand_tilde(p)
}

fn canonical_or_self(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

fn starts_with(candidate: &Path, prefix: &Path) -> bool {
    let c = canonical_or_self(candidate);
    let p = canonical_or_self(prefix);
    c.starts_with(&p)
}

fn expand_fs(fs: &mut FsSection) {
    for r in fs.roots.iter_mut() {
        r.path = expand_tilde(&r.path);
    }
    for d in fs.denied.iter_mut() {
        *d = expand_tilde(d);
    }
}


pub fn load() -> HumConfig {
    let path = config_path();
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return HumConfig::default(),
        Err(e) => {
            warn!(path = %path.display(), error = %e, "config.read.failed");
            return HumConfig::default();
        }
    };
    let mut cfg = match serde_json::from_str::<HumConfig>(&raw) {
        Ok(cfg) => cfg,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "config.parse.failed");
            return HumConfig::default();
        }
    };
    expand_fs(&mut cfg.fs);
    cfg
}

pub const SCHEMA: &str = include_str!("../../hum.schema.json");

pub const RETIRED: &[(&str, &str)] = &[("humd", "driftRetentionDays")];

pub fn drop_retired(instance: &mut serde_json::Value) -> Vec<String> {
    let mut dropped = Vec::new();
    for (section, key) in RETIRED {
        let Some(obj) = instance.get_mut(*section).and_then(|s| s.as_object_mut()) else {
            continue;
        };
        if obj.remove(*key).is_some() {
            dropped.push(format!("{section}.{key}"));
        }
    }
    dropped
}

pub fn retired_in_raw(raw: &str) -> Vec<String> {
    let Ok(mut instance) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Vec::new();
    };
    drop_retired(&mut instance)
}

pub fn retired_in_file() -> Vec<String> {
    let path = config_path();
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    retired_in_raw(&raw)
}

pub fn migrate_file() -> Vec<String> {
    let path = config_path();
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(mut instance) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    let dropped = drop_retired(&mut instance);
    if dropped.is_empty() { return Vec::new(); }
    if let Ok(mut pretty) = serde_json::to_string_pretty(&instance) {
        pretty.push('\n');
        let _ = std::fs::write(&path, pretty);
    }
    dropped
}

pub fn validate(raw: &str) -> Result<(), Vec<String>> {
    let mut instance: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| vec![format!("invalid JSON: {e}")])?;
    drop_retired(&mut instance);
    let schema: serde_json::Value = serde_json::from_str(SCHEMA)
        .expect("embedded hum.schema.json is itself valid JSON");
    let validator = jsonschema::validator_for(&schema)
        .expect("embedded hum.schema.json compiles");
    let errors: Vec<String> = validator
        .iter_errors(&instance)
        .map(|e| {
            let at = e.instance_path().to_string();
            if at.is_empty() || at == "/" { e.to_string() } else { format!("{e} (at {at})") }
        })
        .collect();
    if errors.is_empty() { Ok(()) } else { Err(errors) }
}

pub fn validate_or_exit() {
    let path = config_path();
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            eprintln!("hum.json: cannot read {}: {e}", path.display());
            std::process::exit(1);
        }
    };
    if let Err(violations) = validate(&raw) {
        eprintln!("hum.json failed schema validation ({}):", path.display());
        for v in &violations {
            eprintln!("  - {v}");
        }
        eprintln!("\nFix the file and restart. Schema: hum.schema.json");
        std::process::exit(1);
    }
}

mod defaults {
    use std::path::PathBuf;

    pub(crate) fn permission_dusk_ms() -> u64 {
        60_000
    }
    pub(crate) fn metrics_addr() -> String {
        "127.0.0.1:9909".into()
    }
    pub(crate) fn max_active_cells() -> u32 {
        4
    }
    pub(crate) fn cell_idle_prune_threshold_ms() -> u64 {
        300_000
    }
    pub(crate) fn default_hive() -> String {
        "claude-repl".into()
    }
    pub(crate) fn denied() -> Vec<PathBuf> {
        vec![
            PathBuf::from("~/.ssh"),
            PathBuf::from("~/.aws"),
            PathBuf::from("~/.gnupg"),
            crate::config_dir(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_yields_defaults() {
        let cfg: HumConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.nest.max_active_cells, 4);
        assert_eq!(cfg.nest.cell_idle_prune_threshold_ms, 300_000);
        assert_eq!(cfg.nest.default, "claude-repl");
        assert_eq!(cfg.humd.permission_dusk_ms, 60_000);
    }

    #[test]
    fn nest_section_partial_takes_defaults() {
        let cfg: HumConfig =
            serde_json::from_str(r#"{"nest": {"maxActiveCells": 8, "default": "claude-cli"}}"#).unwrap();
        assert_eq!(cfg.nest.max_active_cells, 8);
        assert_eq!(cfg.nest.default, "claude-cli");
        let default_threshold = HumConfig::default().nest.cell_idle_prune_threshold_ms;
        assert_eq!(cfg.nest.cell_idle_prune_threshold_ms, default_threshold);
    }

    #[test]
    fn schema_accepts_canonical_config() {
        let ok = r#"{
            "$schema": "https://adiled.github.io/hum/hum.schema.json",
            "humd": { "permissionDuskMs": 60000 },
            "fs": { "roots": [ { "path": "~/code", "mode": "rw" } ], "denied": [] },
            "nest": { "maxActiveCells": 4, "cellIdlePruneThresholdMs": 300000, "default": "claude-cli" }
        }"#;
        assert!(validate(ok).is_ok(), "canonical config should validate: {:?}", validate(ok));
    }

    #[test]
    fn schema_rejects_legacy_hives_block() {
        let bad = r#"{
            "nest": { "default": "claude-cli" },
            "hives": { "claude-cli": { "bin": "claude", "flags": [], "model": "opus" } }
        }"#;
        assert!(validate(bad).is_err(), "legacy hives block must fail schema validation");
    }

    #[test]
    fn retired_key_does_not_fail_validation() {
        let stale = r#"{
            "humd": { "permissionDuskMs": 60000, "driftRetentionDays": 30 }
        }"#;
        assert!(validate(stale).is_ok(), "a retired key must not brick the daemon: {:?}", validate(stale));
        assert_eq!(retired_in_raw(stale), vec!["humd.driftRetentionDays".to_string()]);
    }

    #[test]
    fn unknown_key_still_fails_validation() {
        let typo = r#"{ "humd": { "permissionDuskMsec": 60000 } }"#;
        assert!(validate(typo).is_err(), "a key that was never known is still a violation");
    }

    #[test]
    fn fs_root_accepts_modes() {
        let cfg: HumConfig = serde_json::from_str(
            r#"{ "fs": { "roots": [ { "path": "/tmp", "mode": "ro" } ] } }"#,
        )
        .unwrap();
        assert_eq!(cfg.fs.roots.len(), 1);
        assert_eq!(cfg.fs.roots[0].mode, FsMode::Ro);
    }

    #[test]
    fn fs_check_allows_inside_root_denies_outside() {
        let mut fs = FsSection {
            roots: vec![FsRoot { path: PathBuf::from("/tmp"), mode: FsMode::Rw }],
            denied: vec![],
        };
        expand_fs(&mut fs);
        assert!(matches!(fs.check(Path::new("/tmp")), Ok(FsMode::Rw)));
        assert!(matches!(fs.check(Path::new("/etc/passwd")), Err(FsDenial::OutsideRoots)));
    }

    #[test]
    fn fs_check_denied_overrides_root() {
        let mut fs = FsSection {
            roots: vec![FsRoot { path: PathBuf::from("/tmp"), mode: FsMode::Rw }],
            denied: vec![PathBuf::from("/tmp/secret")],
        };
        expand_fs(&mut fs);
        std::fs::create_dir_all("/tmp/secret").ok();
        assert!(matches!(fs.check(Path::new("/tmp/secret")), Err(FsDenial::Blacklisted(_))));
    }

    #[test]
    fn serde_is_lenient_but_schema_is_strict() {
        let raw = r#"{"droned": true, "smallModel": "x"}"#;
        assert!(serde_json::from_str::<HumConfig>(raw).is_ok());
        assert!(validate(raw).is_err());
    }
}

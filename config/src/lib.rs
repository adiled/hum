//! hum.json loader (0.3).
//!
//! Namespaced: humd / fs / nest / perches / nestlings. Each section
//! deserializes into its own struct so a perch crate can ship its own
//! `Default` without humd's crate knowing about it.
//!
//! Schema-tolerant: missing sections fill with defaults, parse errors
//! warn and fall back to defaults — never fatal at startup.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::warn;

// ── humd ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumdSection {
    #[serde(default = "defaults::permission_dusk_ms", rename = "permissionDuskMs")]
    pub permission_dusk_ms: u64,
    #[serde(default = "defaults::drift_retention_days", rename = "driftRetentionDays")]
    pub drift_retention_days: u32,
}

impl Default for HumdSection {
    fn default() -> Self {
        Self {
            permission_dusk_ms: defaults::permission_dusk_ms(),
            drift_retention_days: defaults::drift_retention_days(),
        }
    }
}

// ── fs ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FsMode {
    Rw,
    Ro,
}

impl Default for FsMode {
    fn default() -> Self {
        FsMode::Rw
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsRoot {
    /// Tilde-expanded at load time. Always stored as an absolute path.
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
    /// Resolve a candidate path against the fs policy.
    ///
    /// Returns `Ok(FsMode)` when the path is permitted (with the
    /// effective mode of its enclosing root). Returns `Err(reason)`
    /// when the path is denied or sits outside every root.
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
    /// Path matched an entry in `fs.denied`.
    Blacklisted(PathBuf),
    /// Path doesn't sit inside any configured root.
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

// ── nest ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NestSection {
    #[serde(default = "defaults::max_procs", rename = "maxProcs")]
    pub max_procs: u32,
    #[serde(default = "defaults::idle_threshold_ms", rename = "idleThresholdMs")]
    pub idle_threshold_ms: u64,
    #[serde(default = "defaults::default_perch")]
    pub default: String,
}

impl Default for NestSection {
    fn default() -> Self {
        Self {
            max_procs: defaults::max_procs(),
            idle_threshold_ms: defaults::idle_threshold_ms(),
            default: defaults::default_perch(),
        }
    }
}

// ── perches ───────────────────────────────────────────────────────────────

/// Per-perch config. Schema-loose: each perch crate parses the `Value`
/// against its own `Config` struct. Common fields recognized here so the
/// top-level loader can validate the shape of the obvious cases.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PerchConfig {
    #[serde(default, rename = "cliPath")]
    pub cli_path: Option<String>,
    #[serde(default, rename = "defaultModel")]
    pub default_model: Option<String>,
    #[serde(default, rename = "ccFlags")]
    pub cc_flags: BTreeMap<String, Value>,
    #[serde(default)]
    pub limits: Option<PerchLimits>,
    #[serde(default)]
    pub budget: Option<PerchBudget>,
    /// Anything else the perch's own deserializer handles.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerchLimits {
    #[serde(default)]
    pub rss_bytes: Option<u64>,
    #[serde(default)]
    pub fd_count: Option<u32>,
    #[serde(default)]
    pub cpu_secs: Option<u32>,
    #[serde(default)]
    pub wall_clock_ms: Option<u64>,
    #[serde(default)]
    pub nice: Option<i32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerchBudget {
    #[serde(default)]
    pub tokens_per_turn: Option<u64>,
    #[serde(default)]
    pub tokens_per_day: Option<u64>,
    #[serde(default)]
    pub tool_calls_per_minute: Option<u32>,
}

// ── nestlings ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NestlingConfig {
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default, rename = "apiKey")]
    pub api_key: Option<String>,
    /// Anything else the nestling's own deserializer handles.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

// ── top-level ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HumConfig {
    #[serde(default)]
    pub humd: HumdSection,
    #[serde(default)]
    pub fs: FsSection,
    #[serde(default)]
    pub nest: NestSection,
    #[serde(default)]
    pub perches: BTreeMap<String, PerchConfig>,
    #[serde(default)]
    pub nestlings: BTreeMap<String, NestlingConfig>,
}

// ── path resolution ───────────────────────────────────────────────────────

pub fn config_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("hum").join("hum.json");
        }
    }
    if let Some(base) = BaseDirs::new() {
        return base.config_dir().join("hum").join("hum.json");
    }
    PathBuf::from(".config/hum/hum.json")
}

/// Expand `~` against `$HOME`. Leaves absolute / non-tilde paths alone.
fn expand_tilde(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    if s == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home);
        }
    }
    p.to_path_buf()
}

fn canonical_or_self(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

fn starts_with(candidate: &Path, prefix: &Path) -> bool {
    let c = canonical_or_self(candidate);
    let p = canonical_or_self(prefix);
    c.starts_with(&p)
}

/// After deserialize, walk fs paths and expand tildes. Done at load time
/// so every downstream check_against_fs sees absolute paths.
fn expand_fs(fs: &mut FsSection) {
    for r in fs.roots.iter_mut() {
        r.path = expand_tilde(&r.path);
    }
    for d in fs.denied.iter_mut() {
        *d = expand_tilde(d);
    }
}

// ── load ──────────────────────────────────────────────────────────────────

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

mod defaults {
    use std::path::PathBuf;

    pub fn permission_dusk_ms() -> u64 {
        60_000
    }
    pub fn drift_retention_days() -> u32 {
        30
    }
    pub fn max_procs() -> u32 {
        4
    }
    pub fn idle_threshold_ms() -> u64 {
        300_000
    }
    pub fn default_perch() -> String {
        "claude-repl".into()
    }
    pub fn denied() -> Vec<PathBuf> {
        [
            "~/.ssh",
            "~/.aws",
            "~/.gnupg",
            "~/.config/hum",
        ]
        .iter()
        .map(PathBuf::from)
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_yields_defaults() {
        let cfg: HumConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.nest.max_procs, 4);
        assert_eq!(cfg.nest.idle_threshold_ms, 300_000);
        assert_eq!(cfg.nest.default, "claude-repl");
        assert_eq!(cfg.humd.permission_dusk_ms, 60_000);
        assert!(cfg.perches.is_empty());
        assert!(cfg.nestlings.is_empty());
    }

    #[test]
    fn nest_section_partial() {
        let cfg: HumConfig =
            serde_json::from_str(r#"{"nest": {"maxProcs": 8, "default": "claude-cli"}}"#).unwrap();
        assert_eq!(cfg.nest.max_procs, 8);
        assert_eq!(cfg.nest.default, "claude-cli");
        assert_eq!(cfg.nest.idle_threshold_ms, 300_000); // default fills
    }

    #[test]
    fn perches_each_have_their_own_subobject() {
        let cfg: HumConfig = serde_json::from_str(
            r#"{
                "perches": {
                    "claude-cli":  { "cliPath": "/usr/bin/claude", "defaultModel": "claude-sonnet-4-5" },
                    "claude-repl": { "defaultModel": "claude-haiku-4-5" }
                }
            }"#,
        )
        .unwrap();
        assert_eq!(cfg.perches["claude-cli"].cli_path.as_deref(), Some("/usr/bin/claude"));
        assert_eq!(cfg.perches["claude-cli"].default_model.as_deref(), Some("claude-sonnet-4-5"));
        assert_eq!(cfg.perches["claude-repl"].default_model.as_deref(), Some("claude-haiku-4-5"));
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
        // Inside root
        assert!(matches!(fs.check(Path::new("/tmp")), Ok(FsMode::Rw)));
        // Outside any root
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
    fn unknown_section_keys_are_ignored() {
        let cfg: Result<HumConfig, _> =
            serde_json::from_str(r#"{"droned": true, "smallModel": "x"}"#);
        // Old-shape keys aren't recognized at root but serde's #[serde(default)]
        // on each section means we get a default config, not a parse error.
        // (Strict-mode validation lives in the schema, not in serde.)
        let cfg = cfg.unwrap();
        assert!(!cfg.perches.contains_key("droned"));
    }
}

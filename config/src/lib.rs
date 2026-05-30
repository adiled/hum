//! hum.json loader (0.3).
//!
//! Namespaced daemon policy: `humd` / `fs` / `nest`. Missing sections
//! fill with defaults. Validity is defined by `hum.schema.json`
//! ([`SCHEMA`]); [`validate_or_exit`] gates daemon startup on it, so an
//! invalid config is fatal rather than silently coerced.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
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

// ── top-level ─────────────────────────────────────────────────────────────

/// Daemon-scoped policy: `humd` knobs, `fs` grounding, and `nest`
/// routing/capacity. A hive's own runtime (binary, models, flags) is
/// owned by the hive process via its service env, not by humd.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HumConfig {
    #[serde(default)]
    pub humd: HumdSection,
    #[serde(default)]
    pub fs: FsSection,
    #[serde(default)]
    pub nest: NestSection,
}

// ── path resolution ───────────────────────────────────────────────────────

pub fn config_path() -> PathBuf {
    hum_paths::hum_json()
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

/// Surface config keys humd silently ignores. The valid top level is
/// `$schema`, `humd`, `fs`, `nest`. Anything else parses fine and does
/// nothing — most often a `hives`/`perches`/`nestlings` block, which is
/// the wrong place: a hive configures itself through its service env.
/// Returns one human-readable line per finding. Called at boot (logged
/// as `config.lint`) and by `hum doctor`.
/// The canonical schema, embedded at build time. The single source of
/// truth for what a valid hum.json is.
pub const SCHEMA: &str = include_str!("../../hum.schema.json");

/// Validate raw hum.json text against [`SCHEMA`]. Returns every
/// violation verbatim (the schema's own messages), or `Ok` if valid.
pub fn validate(raw: &str) -> Result<(), Vec<String>> {
    let instance: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| vec![format!("invalid JSON: {e}")])?;
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

/// Gate daemon startup on a schema-valid config. A daemon must not boot
/// on an invalid config; we surface the violations verbatim and exit
/// non-zero. A missing file is fine (defaults apply).
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

    pub fn permission_dusk_ms() -> u64 {
        60_000
    }
    pub fn drift_retention_days() -> u32 {
        30
    }
    pub fn max_active_cells() -> u32 {
        4
    }
    pub fn cell_idle_prune_threshold_ms() -> u64 {
        300_000
    }
    pub fn default_hive() -> String {
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
        assert_eq!(cfg.nest.max_active_cells, 4);
        assert_eq!(cfg.nest.cell_idle_prune_threshold_ms, 300_000);
        assert_eq!(cfg.nest.default, "claude-repl");
        assert_eq!(cfg.humd.permission_dusk_ms, 60_000);
    }

    #[test]
    fn nest_section_partial() {
        let cfg: HumConfig =
            serde_json::from_str(r#"{"nest": {"maxActiveCells": 8, "default": "claude-cli"}}"#).unwrap();
        assert_eq!(cfg.nest.max_active_cells, 8);
        assert_eq!(cfg.nest.default, "claude-cli");
        assert_eq!(cfg.nest.cell_idle_prune_threshold_ms, 300_000); // default fills
    }

    #[test]
    fn schema_accepts_canonical_config() {
        let ok = r#"{
            "$schema": "https://adiled.github.io/hum/hum.schema.json",
            "humd": { "permissionDuskMs": 60000, "driftRetentionDays": 30 },
            "fs": { "roots": [ { "path": "~/code", "mode": "rw" } ], "denied": [] },
            "nest": { "maxActiveCells": 4, "cellIdlePruneThresholdMs": 300000, "default": "claude-cli" }
        }"#;
        assert!(validate(ok).is_ok(), "canonical config should validate: {:?}", validate(ok));
    }

    #[test]
    fn schema_rejects_legacy_hives_block() {
        // The exact drift that silently broke setups before: a hives
        // block (or perches/nestlings, or junk hive keys) must now fail
        // validation, not parse-and-ignore.
        let bad = r#"{
            "nest": { "default": "claude-cli" },
            "hives": { "claude-cli": { "bin": "claude", "flags": [], "model": "opus" } }
        }"#;
        assert!(validate(bad).is_err(), "legacy hives block must fail schema validation");
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
    fn serde_is_lenient_but_schema_is_strict() {
        // serde with #[serde(default)] tolerates unknown keys (parses to
        // a default config, no error) — that's why validity is enforced
        // by the schema, not serde. The schema rejects the same input.
        let raw = r#"{"droned": true, "smallModel": "x"}"#;
        assert!(serde_json::from_str::<HumConfig>(raw).is_ok());
        assert!(validate(raw).is_err());
    }
}

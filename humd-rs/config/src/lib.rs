//! hum.json loader. XDG-located, schema-tolerant, defaults silently fill gaps.
//!
//! Mirrors `lib/config.ts` — the TypeScript daemon and this crate must agree
//! on the wire shape so a hum.json written by either side round-trips.

use std::collections::BTreeMap;
use std::path::PathBuf;

use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use tracing::warn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DroneModel {
    #[serde(rename = "providerID")]
    pub provider_id: String,
    #[serde(rename = "modelID")]
    pub model_id: String,
}

impl Default for DroneModel {
    fn default() -> Self {
        Self {
            provider_id: "opencode-hum".into(),
            model_id: "claude-haiku-4-5".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    #[serde(rename = "primaryPath")]
    pub primary_path: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Experimental {
    #[serde(default)]
    pub subpath: bool,
}

/// Default nest implementation. `claude-repl` drives the Ink REPL through a
/// PTY (Pro/Max subscription). `claude-cli` is the legacy `-p` headless mode
/// (API credits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Nest {
    #[serde(rename = "claude-repl")]
    ClaudeRepl,
    #[serde(rename = "claude-cli")]
    ClaudeCli,
}

impl Default for Nest {
    fn default() -> Self {
        Nest::ClaudeRepl
    }
}

/// Manual compaction behavior for hum-routed sessions. Auto-compaction is
/// permanently off (models declare `limit.context: 0`); this only governs the
/// TUI's manual compact button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Compaction {
    #[serde(rename = "off")]
    Off,
    #[serde(rename = "curate")]
    Curate,
}

impl Default for Compaction {
    fn default() -> Self {
        Compaction::Off
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumConfig {
    #[serde(default = "defaults::max_procs", rename = "maxProcs")]
    pub max_procs: u32,

    #[serde(default = "defaults::idle_timeout", rename = "idleTimeout")]
    pub idle_timeout: u64,

    #[serde(default, rename = "smallModel")]
    pub small_model: String,

    #[serde(default = "defaults::permission_dusk", rename = "permissionDusk")]
    pub permission_dusk: u64,

    #[serde(default)]
    pub droned: bool,

    #[serde(default, rename = "droneModel")]
    pub drone_model: DroneModel,

    #[serde(default)]
    pub nest: Nest,

    #[serde(default)]
    pub projects: Vec<Project>,

    #[serde(default)]
    pub experimental: Experimental,

    #[serde(default, rename = "ccFlags")]
    pub cc_flags: BTreeMap<String, String>,

    #[serde(default)]
    pub compaction: Compaction,

    #[serde(default = "defaults::drift_retention_days", rename = "driftRetentionDays")]
    pub drift_retention_days: u32,
}

mod defaults {
    pub fn max_procs() -> u32 {
        4
    }
    pub fn idle_timeout() -> u64 {
        30_000
    }
    pub fn permission_dusk() -> u64 {
        60_000
    }
    pub fn drift_retention_days() -> u32 {
        30
    }
}

impl Default for HumConfig {
    fn default() -> Self {
        Self {
            max_procs: defaults::max_procs(),
            idle_timeout: defaults::idle_timeout(),
            small_model: String::new(),
            permission_dusk: defaults::permission_dusk(),
            droned: false,
            drone_model: DroneModel::default(),
            nest: Nest::default(),
            projects: Vec::new(),
            experimental: Experimental::default(),
            cc_flags: BTreeMap::new(),
            compaction: Compaction::default(),
            drift_retention_days: defaults::drift_retention_days(),
        }
    }
}

/// Resolve ${XDG_CONFIG_HOME or $HOME/.config}/hum/hum.json.
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

/// Best-effort load. Missing file, parse errors, and shape mismatches all
/// fall through to defaults — warnings logged via `tracing` but never fatal.
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
    match serde_json::from_str::<HumConfig>(&raw) {
        Ok(cfg) => cfg,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "config.parse.failed");
            HumConfig::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_ts() {
        let d = HumConfig::default();
        assert_eq!(d.max_procs, 4);
        assert_eq!(d.idle_timeout, 30_000);
        assert_eq!(d.permission_dusk, 60_000);
        assert_eq!(d.nest, Nest::ClaudeRepl);
        assert_eq!(d.compaction, Compaction::Off);
        assert_eq!(d.drift_retention_days, 30);
        assert!(!d.droned);
        assert_eq!(d.drone_model.provider_id, "opencode-hum");
        assert_eq!(d.drone_model.model_id, "claude-haiku-4-5");
    }

    #[test]
    fn empty_json_yields_defaults() {
        let cfg: HumConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.max_procs, 4);
        assert_eq!(cfg.nest, Nest::ClaudeRepl);
    }

    #[test]
    fn partial_overrides() {
        let cfg: HumConfig =
            serde_json::from_str(r#"{"maxProcs": 8, "nest": "claude-cli"}"#).unwrap();
        assert_eq!(cfg.max_procs, 8);
        assert_eq!(cfg.nest, Nest::ClaudeCli);
        assert_eq!(cfg.idle_timeout, 30_000);
    }
}

//! A hive's nest: the cells it hosts and the limits it keeps them under.
//!
//! humd hosts no cells. A hive that does turns this module on, and the
//! limits are the hive's own, in the `nest` section of its
//! `hives/<hive>.json`. Without one, a hive gets the limits every hum-nest
//! worker ran with before a hive could say otherwise. The headroom left
//! under those limits is what the worker claims to humd.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::warn;

use crate::limits::Bounds;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NestConfig {
    pub max_cells: usize,
    pub idle_ms: u64,
    pub bounds: Bounds,
}

impl Default for NestConfig {
    fn default() -> Self {
        Self {
            max_cells: 8,
            idle_ms: 300_000,
            bounds: Bounds::default(),
        }
    }
}

impl NestConfig {
    pub fn for_hive(hive: &str) -> Self {
        let path = hum_paths::hive_config(hive);
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        Self::from_hive_config(&raw).unwrap_or_else(|e| {
            warn!(path = %path.display(), err = %e, "nest.config.invalid — using the default limits");
            Self::default()
        })
    }

    pub fn from_hive_config(raw: &str) -> serde_json::Result<Self> {
        let config: Value = serde_json::from_str(raw)?;
        match config.get("nest") {
            Some(nest) => serde_json::from_value(nest.clone()),
            None => Ok(Self::default()),
        }
    }

    pub fn max_cells(&self) -> usize {
        self.max_cells.max(1)
    }

    pub fn headroom(&self, live: usize) -> Value {
        let total = self.max_cells();
        json!({"freeSlots": total.saturating_sub(live), "totalSlots": total})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hive_without_a_nest_section_gets_the_defaults() {
        assert_eq!(NestConfig::from_hive_config("{}").unwrap(), NestConfig::default());
        assert_eq!(NestConfig::from_hive_config(r#"{"defaultModel": "x"}"#).unwrap(), NestConfig::default());
    }

    #[test]
    fn a_hive_says_its_own_limits_and_the_rest_default() {
        let cfg = NestConfig::from_hive_config(r#"{"nest": {"maxCells": 2, "bounds": {"rssBytes": 1048576}}}"#).unwrap();
        assert_eq!(cfg.max_cells, 2);
        assert_eq!(cfg.idle_ms, NestConfig::default().idle_ms);
        assert_eq!(cfg.bounds.rss_bytes, Some(1_048_576));
    }

    #[test]
    fn headroom_is_what_is_left_under_the_limit() {
        let cfg = NestConfig { max_cells: 2, ..NestConfig::default() };
        assert_eq!(cfg.headroom(0), json!({"freeSlots": 2, "totalSlots": 2}));
        assert_eq!(cfg.headroom(2), json!({"freeSlots": 0, "totalSlots": 2}));
        assert_eq!(cfg.headroom(5), json!({"freeSlots": 0, "totalSlots": 2}));
        let none = NestConfig { max_cells: 0, ..NestConfig::default() };
        assert_eq!(none.headroom(0), json!({"freeSlots": 1, "totalSlots": 1}));
    }
}

//! Comb — the kernel's measure of what crossed it.
//!
//! A comb is a ledger of quantities: a flat map of named counters that
//! accumulates for the life of the install. Two families live in it.
//!
//! **Activity** — what crossed the kernel, by chi: `chi:wilt`,
//! `chi:tendril-reach`, and `hive:<hive>:chi:wilt` for the same count
//! scoped to the hive of the bee that did the work. The scoped counts
//! cover the work chis: a bloom wilted, a tendril reached, a tendril
//! answered, something went wrong, because those are the ones that name
//! what was done.
//!
//! **Nectar** — what was consumed, by metric: `nectar:in`, `nectar:out`,
//! `nectar:cacheRead`, `nectar:cacheCreate`, and the same scoped to a hive
//! as `hive:<hive>:nectar:in`. Nectar is the kernel's raw intake:
//! tokens in, tokens out, cache reads and writes. A hive may name its
//! tokens any of the ways the wilds do: `input_tokens` and `prompt_tokens`
//! are the same measure, and so are their output counterparts. What the
//! kernel produces is honey, and that side belongs to whoever earns it.
//!
//! Only the kernel writes the comb. The daemon sees every tone that
//! crosses it, so the measure is observed rather than self-reported: a bee
//! metering itself would be reporting on itself. A tone that crossed the
//! mesh has no bee here to answer for it, so it lands in the global family
//! alone. Each humd keeps its own comb, so a sigil that touches two
//! humds is accounted by each of them rather than counted once.
//!
//! A wilt carries its bloom's nectar, and nectar is read from nothing
//! else. Nothing is measured that was not measured: a hive that reports
//! no nectar is measured as having used nothing.
//!
//! The scope is the hive, not the bee: bees come and go and rename
//! themselves, while the kind is what an operator asks about. Older
//! combs wrote the same scope as `bee:<hive>:`.
//!
//! Persisted at `${XDG_RUNTIME_DIR}/hum/comb.json`, write-through every
//! `comb_persist_interval`.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use serde_json::Value;
use tracing::{trace, warn};

pub const WORK_CHIS: [&str; 4] = ["wilt", "tendril-reach", "tendril-result", "error"];

pub const TOKEN_METRICS: [(&str, &[&str]); 4] = [
    (
        "in",
        &[
            "input_tokens",
            "inputTokens",
            "prompt_tokens",
            "promptTokens",
        ],
    ),
    (
        "out",
        &[
            "output_tokens",
            "outputTokens",
            "completion_tokens",
            "completionTokens",
        ],
    ),
    (
        "cacheRead",
        &[
            "cache_read_input_tokens",
            "cacheReadInputTokens",
            "cached_tokens",
        ],
    ),
    (
        "cacheCreate",
        &["cache_creation_input_tokens", "cacheCreationInputTokens"],
    ),
];

pub fn chi_key(chi: &str) -> String {
    format!("chi:{chi}")
}

pub fn hive_chi_key(hive: &str, chi: &str) -> String {
    format!("hive:{hive}:chi:{chi}")
}

pub fn nectar_key(metric: &str) -> String {
    format!("nectar:{metric}")
}

pub fn hive_nectar_key(hive: &str, metric: &str) -> String {
    format!("hive:{hive}:nectar:{metric}")
}

fn rescoped(key: String) -> String {
    match key.strip_prefix("bee:") {
        Some(rest) => format!("hive:{rest}"),
        None => key,
    }
}

pub fn nectar_of(nectar: Option<&Value>) -> Vec<(&'static str, u64)> {
    let Some(obj) = nectar.and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (metric, names) in TOKEN_METRICS {
        for name in names {
            if let Some(n) = obj.get(*name).and_then(Value::as_u64) {
                out.push((metric, n));
                break;
            }
        }
    }
    out
}

#[derive(Debug)]
pub struct Comb {
    counters: RwLock<HashMap<String, u64>>,
}

impl Comb {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            counters: RwLock::new(HashMap::new()),
        })
    }

    pub fn record(&self, chi: &str, hive: Option<&str>, nectar: Option<&Value>) {
        self.incr(&chi_key(chi));
        let tokens = if chi == "wilt" {
            nectar_of(nectar)
        } else {
            Vec::new()
        };
        for (metric, n) in &tokens {
            self.incr_by(&nectar_key(metric), *n);
        }
        let Some(hive) = hive else {
            return;
        };
        if WORK_CHIS.contains(&chi) {
            self.incr(&hive_chi_key(hive, chi));
        }
        for (metric, n) in &tokens {
            self.incr_by(&hive_nectar_key(hive, metric), *n);
        }
    }

    pub fn load(path: &Path) -> Arc<Self> {
        let counters = match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<HashMap<String, serde_json::Value>>(&bytes)
            {
                Ok(raw) => {
                    let mut out = HashMap::with_capacity(raw.len());
                    for (k, v) in raw {
                        let n = match v.as_u64() {
                            Some(n) => n,
                            None => match v.as_f64() {
                                Some(f) if f.is_finite() && f >= 0.0 => f as u64,
                                _ => continue,
                            },
                        };
                        *out.entry(rescoped(k)).or_insert(0) += n;
                    }
                    trace!(target: "comb", "comb.loaded keys={}", out.len());
                    out
                }
                Err(e) => {
                    warn!(target: "comb", "comb.load.parse_failed err={e}");
                    HashMap::new()
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                trace!(target: "comb", "comb.load.absent");
                HashMap::new()
            }
            Err(e) => {
                warn!(target: "comb", "comb.load.read_failed err={e}");
                HashMap::new()
            }
        };
        let comb = Self::new();
        *comb.counters.write() = counters;
        comb
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent()
            && !dir.as_os_str().is_empty()
        {
            std::fs::create_dir_all(dir)?;
        }
        let snap = self.snapshot();
        let bytes =
            serde_json::to_vec(&snap).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        hum_paths::write_atomic(path, &bytes)?;
        trace!(target: "comb", "comb.saved keys={} bytes={}", snap.len(), bytes.len());
        Ok(())
    }

    pub fn incr(&self, key: &str) {
        self.incr_by(key, 1);
    }

    pub fn incr_by(&self, key: &str, delta: u64) {
        if delta == 0 {
            return;
        }
        let mut g = self.counters.write();
        let slot = g.entry(key.to_string()).or_insert(0);
        *slot = slot.saturating_add(delta);
    }

    pub fn snapshot(&self) -> HashMap<String, u64> {
        self.counters.read().clone()
    }

    pub fn spawn_persister(self: Arc<Self>, path: PathBuf, interval: Duration) {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            tick.tick().await;
            loop {
                tick.tick().await;
                if let Err(e) = self.save(&path) {
                    warn!(target: "comb", "comb.persist.failed err={e}");
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn record_counts_chi_globally() {
        let c = Comb::new();
        c.record("wilt", None, None);
        c.record("wilt", None, None);
        c.record("prompt", None, None);
        let snap = c.snapshot();
        assert_eq!(snap.get("chi:wilt"), Some(&2));
        assert_eq!(snap.get("chi:prompt"), Some(&1));
    }

    #[test]
    fn record_attributes_work_to_the_hive() {
        let c = Comb::new();
        c.record("wilt", Some("claude-cli"), None);
        c.record("tendril-reach", Some("claude-cli"), None);
        c.record("unfurl", Some("claude-cli"), None);
        let snap = c.snapshot();
        assert_eq!(snap.get("hive:claude-cli:chi:wilt"), Some(&1));
        assert_eq!(snap.get("hive:claude-cli:chi:tendril-reach"), Some(&1));
        assert_eq!(snap.get("hive:claude-cli:chi:unfurl"), None);
    }

    #[test]
    fn record_measures_nectar_from_a_wilt() {
        let c = Comb::new();
        let nectar = json!({
            "input_tokens": 1200,
            "output_tokens": 34,
            "cache_read_input_tokens": 900,
        });
        c.record("wilt", Some("claude-cli"), Some(&nectar));
        let snap = c.snapshot();
        assert_eq!(snap.get("nectar:in"), Some(&1200));
        assert_eq!(snap.get("nectar:out"), Some(&34));
        assert_eq!(snap.get("nectar:cacheRead"), Some(&900));
        assert_eq!(snap.get("hive:claude-cli:nectar:in"), Some(&1200));
        assert_eq!(snap.get("hive:claude-cli:nectar:out"), Some(&34));
    }

    #[test]
    fn openai_token_names_measure_the_same_way() {
        let c = Comb::new();
        let nectar = json!({ "prompt_tokens": 10, "completion_tokens": 2 });
        c.record("wilt", None, Some(&nectar));
        let snap = c.snapshot();
        assert_eq!(snap.get("nectar:in"), Some(&10));
        assert_eq!(snap.get("nectar:out"), Some(&2));
    }

    #[test]
    fn nectar_is_only_read_on_a_wilt() {
        let c = Comb::new();
        let nectar = json!({ "input_tokens": 5 });
        c.record("unfurl", Some("claude-cli"), Some(&nectar));
        let snap = c.snapshot();
        assert_eq!(snap.get("nectar:in"), None);
    }

    #[test]
    fn mesh_tones_count_globally_but_are_not_attributed() {
        let c = Comb::new();
        let nectar = json!({ "input_tokens": 7 });
        c.record("wilt", None, Some(&nectar));
        let snap = c.snapshot();
        assert_eq!(snap.get("chi:wilt"), Some(&1));
        assert_eq!(snap.get("nectar:in"), Some(&7));
        assert_eq!(
            snap.keys().filter(|k| k.starts_with("hive:")).count(),
            0,
            "a tone with no bee here must not be attributed to a hive"
        );
    }

    #[test]
    fn a_comb_scoped_by_bee_loads_scoped_by_hive() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("comb.json");
        std::fs::write(
            &path,
            r#"{"bee:claude-cli:chi:wilt":3,"hive:claude-cli:chi:wilt":2,"chi:wilt":5}"#,
        )
        .unwrap();
        let snap = Comb::load(&path).snapshot();
        assert_eq!(snap.get("hive:claude-cli:chi:wilt"), Some(&5));
        assert_eq!(snap.get("bee:claude-cli:chi:wilt"), None);
        assert_eq!(snap.get("chi:wilt"), Some(&5));
    }

    #[test]
    fn incr_and_snapshot() {
        let c = Comb::new();
        c.incr("blooms");
        c.incr("blooms");
        c.incr_by("totalInputTokens", 1234);
        c.incr_by("noop", 0);
        let snap = c.snapshot();
        assert_eq!(snap.get("blooms"), Some(&2));
        assert_eq!(snap.get("totalInputTokens"), Some(&1234));
        assert!(!snap.contains_key("noop"));
    }

    #[test]
    fn save_then_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(hum_paths::COMB_BASENAME);
        let c = Comb::new();
        c.incr_by("curateBytesSaved", 99_999);
        c.incr("taskExecutions");
        c.save(&path).unwrap();

        let c2 = Comb::load(&path);
        let snap = c2.snapshot();
        assert_eq!(snap.get("curateBytesSaved"), Some(&99_999));
        assert_eq!(snap.get("taskExecutions"), Some(&1));
    }

    #[test]
    fn load_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let c = Comb::load(&dir.path().join("does-not-exist.json"));
        assert!(c.snapshot().is_empty());
    }

    #[test]
    fn load_skips_non_numeric_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(hum_paths::COMB_BASENAME);
        std::fs::write(
            &path,
            br#"{"started":1700000000000,"blooms":7,"label":"ignored","fractional":3.7}"#,
        )
        .unwrap();
        let c = Comb::load(&path);
        let snap = c.snapshot();
        assert_eq!(snap.get("blooms"), Some(&7));
        assert_eq!(snap.get("started"), Some(&1_700_000_000_000));
        assert_eq!(snap.get("fractional"), Some(&3));
        assert!(!snap.contains_key("label"));
    }
}

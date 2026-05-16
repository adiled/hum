//! Penny — hum's lifetime counters.
//!
//! A map of named u64 counters, increment-only, persisted as JSON.
//! Nestlers ship `pennyDelta` increments which the daemon merges in.
//!
//! Persisted at `${XDG_STATE_HOME}/hum/penny.json`, write-through every 60s.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use tracing::{trace, warn};

#[derive(Debug)]
pub struct Penny {
    counters: RwLock<HashMap<String, u64>>,
}

impl Penny {
    /// Empty Penny. Use `load` to restore from disk.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            counters: RwLock::new(HashMap::new()),
        })
    }

    /// Read counters from `path`. Missing/corrupt files yield an empty Penny —
    /// counters are increment-only, so losing them is a loss but never a crash.
    pub fn load(path: &Path) -> Arc<Self> {
        let counters = match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<HashMap<String, serde_json::Value>>(&bytes)
            {
                Ok(raw) => {
                    let mut out = HashMap::with_capacity(raw.len());
                    for (k, v) in raw {
                        // Skip non-numeric fields like string labels. Floats truncate to u64.
                        if let Some(n) = v.as_u64() {
                            out.insert(k, n);
                        } else if let Some(f) = v.as_f64() {
                            if f.is_finite() && f >= 0.0 {
                                out.insert(k, f as u64);
                            }
                        }
                    }
                    trace!(target: "penny", "penny.loaded keys={}", out.len());
                    out
                }
                Err(e) => {
                    warn!(target: "penny", "penny.load.parse_failed err={e}");
                    HashMap::new()
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                trace!(target: "penny", "penny.load.absent");
                HashMap::new()
            }
            Err(e) => {
                warn!(target: "penny", "penny.load.read_failed err={e}");
                HashMap::new()
            }
        };
        Arc::new(Self {
            counters: RwLock::new(counters),
        })
    }

    /// Atomically write the current snapshot to `path`. Creates parent dirs.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)?;
            }
        }
        let snap = self.snapshot();
        // Write to a sibling temp file then rename — protects against torn writes
        // when the daemon is killed mid-flush.
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec(&snap)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, path)?;
        trace!(target: "penny", "penny.saved keys={} bytes={}", snap.len(), bytes.len());
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

    /// Bulk merge — typical nestler `pennyDelta` payload.
    pub fn merge(&self, deltas: HashMap<String, u64>) {
        if deltas.is_empty() {
            return;
        }
        let mut g = self.counters.write();
        for (k, v) in deltas {
            if v == 0 {
                continue;
            }
            let slot = g.entry(k).or_insert(0);
            *slot = slot.saturating_add(v);
        }
    }

    pub fn snapshot(&self) -> HashMap<String, u64> {
        self.counters.read().clone()
    }

    /// Background flusher. Owns the Arc; runs until the process exits.
    pub fn spawn_persister(self: Arc<Self>, path: PathBuf, interval: Duration) {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // First tick fires immediately — skip it so we don't double-write on boot.
            tick.tick().await;
            loop {
                tick.tick().await;
                if let Err(e) = self.save(&path) {
                    warn!(target: "penny", "penny.persist.failed err={e}");
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incr_and_snapshot() {
        let p = Penny::new();
        p.incr("blooms");
        p.incr("blooms");
        p.incr_by("totalInputTokens", 1234);
        p.incr_by("noop", 0);
        let snap = p.snapshot();
        assert_eq!(snap.get("blooms"), Some(&2));
        assert_eq!(snap.get("totalInputTokens"), Some(&1234));
        assert!(snap.get("noop").is_none());
    }

    #[test]
    fn merge_sums() {
        let p = Penny::new();
        p.incr_by("reminderStripped", 3);
        let mut delta = HashMap::new();
        delta.insert("reminderStripped".to_string(), 5);
        delta.insert("thrumDedup".to_string(), 2);
        p.merge(delta);
        let snap = p.snapshot();
        assert_eq!(snap.get("reminderStripped"), Some(&8));
        assert_eq!(snap.get("thrumDedup"), Some(&2));
    }

    #[test]
    fn save_then_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/penny.json");
        let p = Penny::new();
        p.incr_by("curateBytesSaved", 99_999);
        p.incr("taskExecutions");
        p.save(&path).unwrap();

        let p2 = Penny::load(&path);
        let snap = p2.snapshot();
        assert_eq!(snap.get("curateBytesSaved"), Some(&99_999));
        assert_eq!(snap.get("taskExecutions"), Some(&1));
    }

    #[test]
    fn load_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let p = Penny::load(&dir.path().join("does-not-exist.json"));
        assert!(p.snapshot().is_empty());
    }

    #[test]
    fn load_skips_non_numeric_fields() {
        // Mirrors the TS shape that included a `started: <ms>` timestamp alongside counters.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("penny.json");
        std::fs::write(
            &path,
            br#"{"started":1700000000000,"blooms":7,"label":"ignored","fractional":3.7}"#,
        )
        .unwrap();
        let p = Penny::load(&path);
        let snap = p.snapshot();
        assert_eq!(snap.get("blooms"), Some(&7));
        assert_eq!(snap.get("started"), Some(&1_700_000_000_000));
        assert_eq!(snap.get("fractional"), Some(&3));
        assert!(snap.get("label").is_none());
    }
}

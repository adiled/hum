//! drift — per-hum timing rings.
//!
//! Each sid keeps a rolling record of:
//! - **marks**: named instants (ms since the bloom opened, first observation wins)
//! - **spans**: named cumulative durations (callable many times, summed)
//! - **flags**: arbitrary key/value attributes (warm, withered, etc.)
//! - **thrum samples**: transit times across the thrum socket, tagged `oc`/`nest`
//!
//! Aggregates are computed lazily across all currently-active blooms and surface
//! p50/p95 per event name. At wilt time (or on demand) blooms append one NDJSON
//! line to `${XDG_STATE_HOME}/hum/drift/YYYY-MM-DD.ndjson`. Older daily files are
//! pruned by `prune_older_than(days)`.
//!
//! Hot path only stamps timestamps — every sort/aggregation happens on read.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThrumSample {
    pub to: String,
    pub ms: u64,
    pub at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bloom {
    pub sid: String,
    pub started_at: u64,
    pub ended_at: Option<u64>,
    pub marks: HashMap<String, u64>,
    pub spans: HashMap<String, u64>,
    pub flags: HashMap<String, Value>,
    pub thrums: Vec<ThrumSample>,
}

impl Bloom {
    fn new(sid: String, now_ms: u64) -> Self {
        Self {
            sid,
            started_at: now_ms,
            ended_at: None,
            marks: HashMap::new(),
            spans: HashMap::new(),
            flags: HashMap::new(),
            thrums: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stat {
    pub n: usize,
    pub p50: u64,
    pub p95: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AggregateReport {
    pub blooms: usize,
    pub marks: HashMap<String, Stat>,
    pub spans: HashMap<String, Stat>,
    pub thrums: HashMap<String, Stat>,
}

#[derive(Debug)]
struct DriftInner {
    active: HashMap<String, Bloom>,
    store_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct Drift {
    inner: Arc<RwLock<DriftInner>>,
}

impl Default for Drift {
    fn default() -> Self {
        Self::new()
    }
}

impl Drift {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(DriftInner {
                active: HashMap::new(),
                store_dir: None,
            })),
        }
    }

    pub fn with_store_dir(dir: impl Into<PathBuf>) -> Self {
        let d = Self::new();
        d.set_store_dir(dir);
        d
    }

    pub fn set_store_dir(&self, dir: impl Into<PathBuf>) {
        let dir = dir.into();
        if let Err(e) = fs::create_dir_all(&dir) {
            tracing::warn!(error = %e, path = %dir.display(), "drift.store.mkdir.failed");
        }
        self.inner.write().store_dir = Some(dir);
    }

    pub fn discard(&self, sid: &str) {
        self.inner.write().active.remove(sid);
    }

    pub fn active_count(&self) -> usize {
        self.inner.read().active.len()
    }

    pub fn mark(&self, sid: &str, event: &str) {
        let now = now_ms();
        let mut g = self.inner.write();
        let b = g
            .active
            .entry(sid.to_string())
            .or_insert_with(|| Bloom::new(sid.to_string(), now));
        b.marks
            .entry(event.to_string())
            .or_insert_with(|| now.saturating_sub(b.started_at));
        tracing::trace!(sid, event, "drift.mark");
    }

    pub fn span(&self, sid: &str, name: &str, ms: u64) {
        let now = now_ms();
        let mut g = self.inner.write();
        let b = g
            .active
            .entry(sid.to_string())
            .or_insert_with(|| Bloom::new(sid.to_string(), now));
        *b.spans.entry(name.to_string()).or_insert(0) += ms;
        tracing::trace!(sid, name, ms, "drift.span");
    }

    pub fn flag(&self, sid: &str, key: &str, value: Value) {
        let now = now_ms();
        let mut g = self.inner.write();
        let b = g
            .active
            .entry(sid.to_string())
            .or_insert_with(|| Bloom::new(sid.to_string(), now));
        b.flags.insert(key.to_string(), value);
    }

    pub fn thrum_sample(&self, sid: &str, direction: &str, ms: u64) {
        const SOFT_CAP: usize = 200;
        let now = now_ms();
        let mut g = self.inner.write();
        let b = g
            .active
            .entry(sid.to_string())
            .or_insert_with(|| Bloom::new(sid.to_string(), now));
        if b.thrums.len() >= SOFT_CAP {
            return;
        }
        let at = now.saturating_sub(b.started_at);
        b.thrums.push(ThrumSample {
            to: direction.to_string(),
            ms,
            at,
        });
    }

    pub fn wilt(&self, sid: &str) -> Option<Bloom> {
        let now = now_ms();
        let mut g = self.inner.write();
        let mut b = g.active.remove(sid)?;
        b.ended_at = Some(now);
        b.marks
            .entry("wilt".to_string())
            .or_insert_with(|| now.saturating_sub(b.started_at));
        let dir = g.store_dir.clone();
        drop(g);
        if let Some(dir) = dir
            && let Err(e) = append_ndjson(&dir, &b)
        {
            tracing::warn!(sid, error = %e, "drift.persist.failed");
        }
        Some(b)
    }

    pub fn aggregates(&self) -> AggregateReport {
        let g = self.inner.read();
        let mut by_mark: HashMap<String, Vec<u64>> = HashMap::new();
        let mut by_span: HashMap<String, Vec<u64>> = HashMap::new();
        let mut by_thrum: HashMap<String, Vec<u64>> = HashMap::new();
        for b in g.active.values() {
            for (k, v) in &b.marks {
                by_mark.entry(k.clone()).or_default().push(*v);
            }
            for (k, v) in &b.spans {
                by_span.entry(k.clone()).or_default().push(*v);
            }
            for s in &b.thrums {
                by_thrum
                    .entry(format!("thrum_to_{}", s.to))
                    .or_default()
                    .push(s.ms);
            }
        }
        AggregateReport {
            blooms: g.active.len(),
            marks: stats(by_mark),
            spans: stats(by_span),
            thrums: stats(by_thrum),
        }
    }

    pub fn persist_today(&self) -> Result<()> {
        let (dir, blooms) = {
            let g = self.inner.read();
            let dir = g
                .store_dir
                .clone()
                .context("drift: store_dir not configured")?;
            let blooms: Vec<Bloom> = g.active.values().cloned().collect();
            (dir, blooms)
        };
        for b in &blooms {
            append_ndjson(&dir, b)?;
        }
        Ok(())
    }

    pub fn prune_older_than(&self, days: u32) -> Result<usize> {
        let dir = match self.inner.read().store_dir.clone() {
            Some(d) => d,
            None => return Ok(0),
        };
        prune_dir(&dir, days)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn day_bucket(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%d").to_string()
}

fn append_ndjson(dir: &Path, b: &Bloom) -> Result<()> {
    let stamp_ms = b.ended_at.unwrap_or(b.started_at);
    let dt = DateTime::<Utc>::from_timestamp_millis(stamp_ms as i64).unwrap_or_else(Utc::now);
    let path = dir.join(format!("{}.ndjson", day_bucket(dt)));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("drift: open {}", path.display()))?;
    let line = serde_json::to_string(b).context("drift: serialize bloom")?;
    f.write_all(line.as_bytes())?;
    f.write_all(b"\n")?;
    Ok(())
}

fn prune_dir(dir: &Path, days: u32) -> Result<usize> {
    if !dir.exists() {
        return Ok(0);
    }
    let cutoff = SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(u64::from(days) * 86_400))
        .unwrap_or(UNIX_EPOCH);
    let mut removed = 0usize;
    for entry in fs::read_dir(dir).with_context(|| format!("drift: read_dir {}", dir.display()))? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("ndjson") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified < cutoff {
            if let Err(e) = fs::remove_file(&path) {
                tracing::warn!(error = %e, path = %path.display(), "drift.prune.unlink.failed");
                continue;
            }
            removed += 1;
        }
    }
    Ok(removed)
}

fn stats(buckets: HashMap<String, Vec<u64>>) -> HashMap<String, Stat> {
    let mut out = HashMap::with_capacity(buckets.len());
    for (k, mut v) in buckets {
        if v.is_empty() {
            continue;
        }
        v.sort_unstable();
        let n = v.len();
        let p50_idx = (n.saturating_mul(50) / 100).min(n - 1);
        let p95_idx = (n.saturating_mul(95) / 100).min(n - 1);
        out.insert(
            k,
            Stat {
                n,
                p50: v[p50_idx],
                p95: v[p95_idx],
            },
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::env;

    fn unique_tmp_dir() -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let mut p = env::temp_dir();
        p.push(format!(
            "drift-test-{}-{}-{}",
            std::process::id(),
            now_ms(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        p
    }

    #[test]
    fn marks_first_only_then_aggregates() {
        let d = Drift::new();
        d.mark("s1", "first_petal");
        std::thread::sleep(std::time::Duration::from_millis(2));
        d.mark("s1", "first_petal"); // ignored
        d.mark("s2", "first_petal");
        let agg = d.aggregates();
        assert_eq!(agg.blooms, 2);
        assert_eq!(agg.marks["first_petal"].n, 2);
    }

    #[test]
    fn spans_accumulate() {
        let d = Drift::new();
        d.span("s1", "reasoning", 10);
        d.span("s1", "reasoning", 25);
        let agg = d.aggregates();
        assert_eq!(agg.spans["reasoning"].n, 1);
        assert_eq!(agg.spans["reasoning"].p50, 35);
    }

    #[test]
    fn thrums_grouped_by_direction() {
        let d = Drift::new();
        d.thrum_sample("s1", "oc", 5);
        d.thrum_sample("s1", "nest", 8);
        d.thrum_sample("s2", "oc", 12);
        let agg = d.aggregates();
        assert_eq!(agg.thrums["thrum_to_oc"].n, 2);
        assert_eq!(agg.thrums["thrum_to_nest"].n, 1);
    }

    #[test]
    fn flag_persists_through_wilt() {
        let dir = unique_tmp_dir();
        let d = Drift::with_store_dir(&dir);
        d.mark("s1", "open");
        d.flag("s1", "warm", json!(true));
        d.flag("s1", "model", json!("opus-4-7"));
        let b = d.wilt("s1").expect("bloom");
        assert_eq!(b.flags["warm"], json!(true));
        assert!(b.marks.contains_key("wilt"));
        let entries: Vec<_> = fs::read_dir(&dir).unwrap().collect();
        assert_eq!(entries.len(), 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn prune_keeps_fresh_files() {
        let dir = unique_tmp_dir();
        let d = Drift::with_store_dir(&dir);
        d.mark("s1", "x");
        d.wilt("s1");
        let removed = d.prune_older_than(1).unwrap();
        assert_eq!(removed, 0);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn persist_today_writes_active_blooms() {
        let dir = unique_tmp_dir();
        let d = Drift::with_store_dir(&dir);
        d.mark("s1", "open");
        d.persist_today().unwrap();
        let entries: Vec<_> = fs::read_dir(&dir).unwrap().collect();
        assert_eq!(entries.len(), 1);
        fs::remove_dir_all(&dir).ok();
    }
}

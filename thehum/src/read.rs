//! Read paths: tail (live), range (file scan), replay (boot reconstruct).

use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;
use tokio::sync::broadcast;

use crate::{Event, Seq, TheHum};

impl TheHum {
    /// Subscribe to new events as they're appended. Lagged subscribers
    /// see a `RecvError::Lagged(n)` if they fall behind by >1024 events.
    pub fn tail(&self) -> broadcast::Receiver<Event> {
        self.live_tx.subscribe()
    }

    /// Stream the historical range [from_seq, ..) for `author`. Includes
    /// events authored by this humd from the local log. (Other authors'
    /// events arrive via gossip; range over the local log is the canonical
    /// answer for THIS humd's history.)
    pub fn range(&self, author: &str, from_seq: Seq) -> Result<Vec<Event>> {
        let mut events = scan_all(&self.dir).context("scan log files")?;
        events.retain(|e| e.author == author && e.seq >= from_seq);
        events.sort_by_key(|e| e.seq);
        Ok(events)
    }

    /// Replay every event in this humd's log (in seq order) through
    /// `handler`. Use to rebuild materialized views on cold-boot.
    /// Handler must be PURE: no clocks, no rng, no env reads — use
    /// `event.ts_ms` for any time-shaped value.
    pub fn replay<F: FnMut(&Event)>(&self, mut handler: F) -> Result<()> {
        let mut events = scan_all(&self.dir).context("scan log files")?;
        events.sort_by_key(|e| (e.author.clone(), e.seq));
        for e in &events {
            handler(e);
        }
        Ok(())
    }
}

fn scan_all(dir: &Path) -> Result<Vec<Event>> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .context("readdir thehum")?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some(hum_paths::THEHUM_NDJSON_EXT))
        .collect();
    files.sort();

    let mut out = Vec::new();
    for path in &files {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() { continue; }
            match serde_json::from_str::<Event>(line) {
                Ok(ev) => out.push(ev),
                Err(e) => tracing::warn!(target: "thehum", path = %path.display(), err = %e, "skip malformed line"),
            }
        }
    }
    Ok(out)
}

/// Verify every event in a vec — signature valid, prev_hash chains,
/// seqs monotonic per author. Returns first violation or Ok(()).
pub fn verify_chain(events: &[Event], pubkey: &ed25519_dalek::VerifyingKey) -> Result<()> {
    let mut last_hash_per_author: std::collections::HashMap<String, [u8; 32]> = Default::default();
    let mut last_seq_per_author: std::collections::HashMap<String, Seq> = Default::default();
    for e in events {
        let v: Value = serde_json::to_value(e)?;
        let canonical = crate::canon::canonical_bytes(&v);
        crate::sign::verify_canonical(pubkey, &canonical, &e.sig)
            .with_context(|| format!("sig invalid at seq {}", e.seq))?;
        let expected_prev = last_hash_per_author.get(&e.author).copied().unwrap_or([0u8; 32]);
        let claimed: Vec<u8> = hex::decode(&e.prev_hash).context("prev_hash hex")?;
        anyhow::ensure!(claimed == expected_prev, "chain break at seq {}", e.seq);
        let expected_seq = last_seq_per_author.get(&e.author).copied().unwrap_or(0) + 1;
        anyhow::ensure!(e.seq == expected_seq, "seq gap at {}: expected {}", e.seq, expected_seq);
        last_seq_per_author.insert(e.author.clone(), e.seq);
        last_hash_per_author.insert(e.author.clone(), crate::sign::hash256(&canonical));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;
    use serde_json::json;
    use tempfile::TempDir;

    #[tokio::test]
    async fn replay_visits_events_in_seq_order() {
        let tmp = TempDir::new().unwrap();
        let key = SigningKey::generate(&mut OsRng);
        let t = TheHum::open(tmp.path(), key, crate::Config::default()).unwrap();
        let rid = crate::HumId::mint();
        for i in 0..5 {
            t.append("e", None, rid.clone(), json!({"i": i})).await.unwrap();
        }
        let mut seen = Vec::new();
        t.replay(|e| seen.push(e.seq)).unwrap();
        assert_eq!(seen, vec![1, 2, 3, 4, 5]);
    }

    #[tokio::test]
    async fn range_returns_from_seq() {
        let tmp = TempDir::new().unwrap();
        let key = SigningKey::generate(&mut OsRng);
        let t = TheHum::open(tmp.path(), key, crate::Config::default()).unwrap();
        let rid = crate::HumId::mint();
        for _ in 0..5 {
            t.append("e", None, rid.clone(), json!({})).await.unwrap();
        }
        let r = t.range(t.author_hid(), 3).unwrap();
        assert_eq!(r.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![3, 4, 5]);
    }

    #[tokio::test]
    async fn tail_receives_subsequent_events() {
        let tmp = TempDir::new().unwrap();
        let key = SigningKey::generate(&mut OsRng);
        let t = TheHum::open(tmp.path(), key, crate::Config::default()).unwrap();
        let mut rx = t.tail();
        let rid = crate::HumId::mint();
        t.append("e", None, rid, json!({})).await.unwrap();
        let ev = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv()).await.unwrap().unwrap();
        assert_eq!(ev.seq, 1);
    }

    #[tokio::test]
    async fn verify_chain_passes_on_clean_log() {
        let tmp = TempDir::new().unwrap();
        let key = SigningKey::generate(&mut OsRng);
        let t = TheHum::open(tmp.path(), key.clone(), crate::Config::default()).unwrap();
        let rid = crate::HumId::mint();
        for _ in 0..3 {
            t.append("e", None, rid.clone(), json!({})).await.unwrap();
        }
        let events = t.range(t.author_hid(), 0).unwrap();
        verify_chain(&events, &key.verifying_key()).expect("clean log verifies");
    }
}

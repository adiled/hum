//! Write path: append, fsync, seq.bin atomic update, hash chain advance.
//!
//! Single appender per humd. Concurrent readers tail via the broadcast
//! channel in TheHum::live_tx; range scans hit the files directly.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::{canon, sign, Event, Hash32, Seq, TheHum};

impl TheHum {
    /// Append a wire envelope as a new authored event. Signs, chains,
    /// fsyncs, broadcasts. Returns the assigned seq.
    pub async fn append(&self, chi: &str, sid: Option<crate::HumId>, rid: crate::HumId, body: Value) -> Result<Seq> {
        let ts_ms = chrono::Utc::now().timestamp_millis();
        let (seq, prev_hash_hex) = {
            let mut s = self.state.lock();
            s.seq += 1;
            (s.seq, hex::encode(s.prev_hash))
        };

        let mut event = Event {
            chi: chi.to_string(),
            sid,
            rid,
            body,
            author: self.author_hid.clone(),
            seq,
            ts_ms,
            prev_hash: prev_hash_hex,
            sig: String::new(),
        };
        let canonical = canon::canonical_bytes_of(&event);
        event.sig = sign::sign_canonical(&self.signing_key, &canonical);

        let this_hash: Hash32 = sign::hash256(&canonical);

        let line = serde_json::to_string(&event).context("serialize event")?;
        let path = daily_path(&self.dir, ts_ms);
        write_line(&path, &line, self.cfg.fsync_per_event)?;
        persist_seq(&self.dir, seq)?;

        {
            let mut s = self.state.lock();
            s.prev_hash = this_hash;
        }

        let _ = self.live_tx.send(event.clone());

        tracing::trace!(target: "thehum", %seq, %chi, "appended");
        Ok(seq)
    }
}

/// Path of the daily ring for the given ts.
pub(crate) fn daily_path(dir: &Path, ts_ms: i64) -> PathBuf {
    let dt = DateTime::<Utc>::from_timestamp_millis(ts_ms)
        .unwrap_or_else(|| DateTime::<Utc>::from_timestamp_millis(0).unwrap());
    let day = dt.format("%Y-%m-%d");
    dir.join(format!("{day}.ndjson"))
}

fn write_line(path: &Path, line: &str, fsync: bool) -> Result<()> {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    f.write_all(line.as_bytes())?;
    f.write_all(b"\n")?;
    if fsync {
        f.sync_data().context("fsync")?;
    }
    Ok(())
}

/// Atomic seq persistence: tmp + rename.
fn persist_seq(dir: &Path, seq: Seq) -> Result<()> {
    let final_path = crate::layout::seq_file(dir);
    let tmp = final_path.with_extension("bin.tmp");
    std::fs::write(&tmp, seq.to_le_bytes())?;
    std::fs::rename(&tmp, &final_path).context("rename seq.bin")?;
    Ok(())
}

/// Cold-boot recovery: read seq.bin and the last line's pre-sig hash.
pub fn recover_state(dir: &Path) -> Result<(Seq, Hash32)> {
    let seq = std::fs::read(crate::layout::seq_file(dir))
        .ok()
        .and_then(|b| {
            if b.len() == 8 {
                let mut arr = [0u8; 8];
                arr.copy_from_slice(&b);
                Some(u64::from_le_bytes(arr))
            } else {
                None
            }
        })
        .unwrap_or(0);

    let mut prev_hash: Hash32 = [0u8; 32];
    if seq > 0 {
        if let Some(line) = last_line_in_dir(dir)? {
            let parsed: Value = serde_json::from_str(&line).context("parse last line")?;
            let canonical = canon::canonical_bytes(&parsed);
            prev_hash = sign::hash256(&canonical);
        }
    }
    Ok((seq, prev_hash))
}

fn last_line_in_dir(dir: &Path) -> Result<Option<String>> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .context("readdir thehum")?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("ndjson"))
        .collect();
    files.sort();
    for path in files.iter().rev() {
        if let Ok(content) = std::fs::read_to_string(path) {
            if let Some(last) = content.lines().filter(|l| !l.trim().is_empty()).last() {
                return Ok(Some(last.to_string()));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;
    use serde_json::json;
    use tempfile::TempDir;

    fn mk_thehum(dir: &Path) -> TheHum {
        let key = SigningKey::generate(&mut OsRng);
        TheHum::open(dir, key, crate::Config::default()).unwrap()
    }

    #[tokio::test]
    async fn append_assigns_monotonic_seq_and_advances_chain() {
        let tmp = TempDir::new().unwrap();
        let t = mk_thehum(tmp.path());
        let rid = crate::HumId::mint();
        let s1 = t.append("prompt", None, rid.clone(), json!({"text": "hi"})).await.unwrap();
        let s2 = t.append("chunk",  None, rid.clone(), json!({"delta": "ok"})).await.unwrap();
        let s3 = t.append("finish", None, rid,         json!({"reason": "end"})).await.unwrap();
        assert_eq!((s1, s2, s3), (1, 2, 3));
    }

    #[tokio::test]
    async fn cold_recover_picks_up_where_we_left_off() {
        let tmp = TempDir::new().unwrap();
        let key = SigningKey::generate(&mut OsRng);
        let first = TheHum::open(tmp.path(), key.clone(), crate::Config::default()).unwrap();
        let rid = crate::HumId::mint();
        first.append("hello", None, rid.clone(), json!({})).await.unwrap();
        first.append("prompt", None, rid, json!({"text": "x"})).await.unwrap();
        drop(first);

        let second = TheHum::open(tmp.path(), key, crate::Config::default()).unwrap();
        let s = second.state.lock();
        assert_eq!(s.seq, 2);
        assert_ne!(s.prev_hash, [0u8; 32], "prev_hash recovered from last line");
    }

    #[tokio::test]
    async fn chain_links_are_consistent() {
        let tmp = TempDir::new().unwrap();
        let t = mk_thehum(tmp.path());
        let rid = crate::HumId::mint();
        t.append("a", None, rid.clone(), json!({})).await.unwrap();
        t.append("b", None, rid.clone(), json!({})).await.unwrap();

        let lines: Vec<String> = std::fs::read_to_string(tmp.path().join(
            crate::append::daily_path(tmp.path(), chrono::Utc::now().timestamp_millis())
                .file_name().unwrap()
        )).unwrap().lines().map(String::from).collect();
        let e0: Value = serde_json::from_str(&lines[0]).unwrap();
        let e1: Value = serde_json::from_str(&lines[1]).unwrap();
        let h0 = sign::hash256(&canon::canonical_bytes(&e0));
        let claimed = hex::decode(e1["prev_hash"].as_str().unwrap()).unwrap();
        assert_eq!(claimed.as_slice(), &h0[..]);
    }
}

//! Snapshots + Merkle root commitments.
//!
//! Periodically the humd computes a Merkle root over its materialized
//! state and emits it as a `chi:"snapshot"` event into its own log.
//! Light peers can sync the root chain without holding full state.

use std::collections::BTreeMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{sign, Hash32, StateRoot, TheHum};

/// One leaf of the state Merkle tree. Materialized views package
/// themselves as (key → bytes) maps before commitment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateLeaf {
    pub key: String,
    pub value: serde_json::Value,
}

/// Build a deterministic Merkle root over a BTreeMap of state leaves.
/// Sorted by key. Tree is binary; odd levels duplicate the last node.
/// Empty tree → all-zero root.
pub fn merkle_root(leaves: &BTreeMap<String, serde_json::Value>) -> StateRoot {
    if leaves.is_empty() {
        return [0u8; 32];
    }
    let mut level: Vec<Hash32> = leaves
        .iter()
        .map(|(k, v)| {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(k.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(&crate::canon::canonical_bytes(v));
            sign::hash256(&bytes)
        })
        .collect();

    while level.len() > 1 {
        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        let mut i = 0;
        while i < level.len() {
            let l = level[i];
            let r = if i + 1 < level.len() { level[i + 1] } else { level[i] };
            let mut combined = [0u8; 64];
            combined[..32].copy_from_slice(&l);
            combined[32..].copy_from_slice(&r);
            next.push(sign::hash256(&combined));
            i += 2;
        }
        level = next;
    }
    level[0]
}

impl TheHum {
    /// Emit a snapshot event for the current materialized state.
    /// `state_leaves` is the projection that THIS humd is committing to.
    /// Call after applying recent events; the committed root is
    /// independent of the chi log itself.
    pub async fn snapshot(&self, state_leaves: BTreeMap<String, serde_json::Value>) -> Result<StateRoot> {
        let root = merkle_root(&state_leaves);
        let height = {
            let s = self.state.lock();
            s.seq
        };
        let body = serde_json::json!({
            "root": hex::encode(root),
            "height": height,
            "leaves": state_leaves.len(),
        });
        self.append("snapshot", None, crate::HumId::mint(), body).await?;
        {
            let mut s = self.state.lock();
            s.last_snapshot_seq = height;
            s.last_snapshot_ts_ms = chrono::Utc::now().timestamp_millis();
        }
        Ok(root)
    }

    /// True when the snapshot cadence policy says we should snapshot.
    /// Cheap; meant to be called per-append or periodically.
    pub fn should_snapshot(&self, now_ms: i64) -> bool {
        let s = self.state.lock();
        let events_since = s.seq.saturating_sub(s.last_snapshot_seq);
        let ms_since = now_ms.saturating_sub(s.last_snapshot_ts_ms);
        events_since >= self.cfg.snapshot_every_events
            || ms_since >= (self.cfg.snapshot_every_seconds as i64) * 1000
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn empty_tree_is_zero_root() {
        let root = merkle_root(&BTreeMap::new());
        assert_eq!(root, [0u8; 32]);
    }

    #[test]
    fn root_is_deterministic_across_insert_order() {
        let mut a = BTreeMap::new();
        a.insert("bee_b".into(), json!({"hid": "b", "online": true}));
        a.insert("bee_a".into(), json!({"hid": "a", "online": true}));
        let mut b = BTreeMap::new();
        b.insert("bee_a".into(), json!({"hid": "a", "online": true}));
        b.insert("bee_b".into(), json!({"hid": "b", "online": true}));
        assert_eq!(merkle_root(&a), merkle_root(&b));
    }

    #[test]
    fn root_changes_on_any_leaf_change() {
        let mut a = BTreeMap::new();
        a.insert("k".into(), json!({"v": 1}));
        let mut b = BTreeMap::new();
        b.insert("k".into(), json!({"v": 2}));
        assert_ne!(merkle_root(&a), merkle_root(&b));
    }

    #[tokio::test]
    async fn snapshot_appends_to_log() {
        let tmp = TempDir::new().unwrap();
        let key = SigningKey::generate(&mut OsRng);
        let t = TheHum::open(tmp.path(), key, crate::Config::default()).unwrap();
        let rid = crate::HumId::mint();
        t.append("e", None, rid, json!({})).await.unwrap();
        let mut state = BTreeMap::new();
        state.insert("sid1".into(), json!({"bees": ["w1"]}));
        let root = t.snapshot(state).await.unwrap();
        assert_ne!(root, [0u8; 32]);

        let events = t.range(t.author_hid(), 0).unwrap();
        assert_eq!(events.last().unwrap().chi, "snapshot");
    }
}

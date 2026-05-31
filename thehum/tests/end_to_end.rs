//! End-to-end integration tests for thehum.
//!
//! Each test drives the public API as a real consumer would: open a
//! dir, append events, drop, reopen, replay, verify. Goal is to prove
//! the protocol holds together end-to-end, not exercise unit-level
//! corners (those live in src/* tests).

use std::collections::{BTreeMap, HashMap};

use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::broadcast::error::TryRecvError;

use thehum::{read::verify_chain, Config, HumId, RetentionMode, TheHum};

fn fresh_key() -> SigningKey {
    SigningKey::generate(&mut OsRng)
}

#[tokio::test]
async fn full_session_lifecycle_replays_to_same_state() {
    let tmp = TempDir::new().unwrap();
    let key = fresh_key();
    let sid = HumId::mint();

    let originals = {
        let t = TheHum::open(tmp.path(), key.clone(), Config::default()).unwrap();
        let rid_prompt = HumId::mint();
        let rid_tool = HumId::mint();

        t.append("hello", Some(sid.clone()), HumId::mint(), json!({"v": 1}))
            .await
            .unwrap();
        t.append("prompt", Some(sid.clone()), rid_prompt.clone(), json!({"text": "hi there"}))
            .await
            .unwrap();
        t.append("chunk", Some(sid.clone()), rid_prompt.clone(), json!({"delta": "he"}))
            .await
            .unwrap();
        t.append("chunk", Some(sid.clone()), rid_prompt.clone(), json!({"delta": "llo"}))
            .await
            .unwrap();
        t.append("chunk", Some(sid.clone()), rid_prompt.clone(), json!({"delta": "!"}))
            .await
            .unwrap();
        t.append(
            "tool-call",
            Some(sid.clone()),
            rid_tool.clone(),
            json!({"name": "fs.read", "args": {"path": "/tmp/x"}}),
        )
        .await
        .unwrap();
        t.append(
            "tool-result",
            Some(sid.clone()),
            rid_tool,
            json!({"ok": true, "bytes": 42}),
        )
        .await
        .unwrap();
        t.append("finish", Some(sid.clone()), rid_prompt, json!({"reason": "end"}))
            .await
            .unwrap();

        t.range(t.author_hid(), 0).unwrap()
    };

    assert_eq!(originals.len(), 8);

    let second = TheHum::open(tmp.path(), key, Config::default()).unwrap();
    let mut seen = Vec::new();
    second.replay(|e| seen.push(e.clone())).unwrap();

    assert_eq!(seen.len(), originals.len(), "replay yields every appended event");
    for (a, b) in originals.iter().zip(seen.iter()) {
        assert_eq!(a.seq, b.seq);
        assert_eq!(a.chi, b.chi);
        assert_eq!(a.body, b.body, "body must round-trip byte-for-byte");
        assert_eq!(a.sid, b.sid);
        assert_eq!(a.ts_ms, b.ts_ms, "ts_ms must flow through replay unchanged");
        assert_eq!(a.author, b.author);
        assert_eq!(a.prev_hash, b.prev_hash);
        assert_eq!(a.sig, b.sig);
    }
}

#[tokio::test]
async fn cross_session_replay_isolates_correctly() {
    let tmp = TempDir::new().unwrap();
    let key = fresh_key();
    let t = TheHum::open(tmp.path(), key, Config::default()).unwrap();

    let sid_a = HumId::mint();
    let sid_b = HumId::mint();

    let order_a = vec!["hello", "prompt", "chunk", "chunk", "finish"];
    let order_b = vec!["hello", "prompt", "tool-call", "tool-result", "finish"];

    // Interleave: a, b, a, b, ...
    for i in 0..order_a.len().max(order_b.len()) {
        if i < order_a.len() {
            t.append(order_a[i], Some(sid_a.clone()), HumId::mint(), json!({"i": i, "side": "a"}))
                .await
                .unwrap();
        }
        if i < order_b.len() {
            t.append(order_b[i], Some(sid_b.clone()), HumId::mint(), json!({"i": i, "side": "b"}))
                .await
                .unwrap();
        }
    }

    let mut buckets: HashMap<HumId, Vec<(u64, String)>> = HashMap::new();
    t.replay(|e| {
        if let Some(s) = e.sid.clone() {
            buckets.entry(s).or_default().push((e.seq, e.chi.clone()));
        }
    })
    .unwrap();

    let a_bucket = buckets.get(&sid_a).expect("sid_a present");
    let b_bucket = buckets.get(&sid_b).expect("sid_b present");

    let a_chis: Vec<&str> = a_bucket.iter().map(|(_, c)| c.as_str()).collect();
    let b_chis: Vec<&str> = b_bucket.iter().map(|(_, c)| c.as_str()).collect();
    assert_eq!(a_chis, order_a, "sid_a history intact in order");
    assert_eq!(b_chis, order_b, "sid_b history intact in order");

    // Seqs must be strictly increasing within each bucket.
    for bucket in [a_bucket, b_bucket] {
        for win in bucket.windows(2) {
            assert!(win[0].0 < win[1].0, "seqs monotonic within sid bucket");
        }
    }
}

#[tokio::test]
async fn tampered_line_fails_verification() {
    let tmp = TempDir::new().unwrap();
    let key = fresh_key();
    let pubkey = key.verifying_key();
    let t = TheHum::open(tmp.path(), key, Config::default()).unwrap();
    let rid = HumId::mint();
    for i in 0..3u32 {
        t.append("e", None, rid.clone(), json!({"i": i})).await.unwrap();
    }

    // Find the daily ndjson, mutate the middle line's body but keep JSON valid.
    let entries: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("ndjson"))
        .collect();
    assert_eq!(entries.len(), 1, "exactly one daily file");
    let path = &entries[0];
    let content = std::fs::read_to_string(path).unwrap();
    let mut lines: Vec<String> = content.lines().map(String::from).collect();
    assert_eq!(lines.len(), 3, "three lines on disk");

    let mut middle: Value = serde_json::from_str(&lines[1]).unwrap();
    middle["body"]["i"] = json!(999);
    lines[1] = serde_json::to_string(&middle).unwrap();
    std::fs::write(path, lines.join("\n") + "\n").unwrap();

    // Re-read events from disk and verify.
    let events = t.range(t.author_hid(), 0).unwrap();
    assert_eq!(events.len(), 3);
    let result = verify_chain(&events, &pubkey);
    assert!(
        result.is_err(),
        "verify_chain must reject tampered event, but returned Ok"
    );
}

#[tokio::test]
async fn snapshot_root_is_stable_across_runs() {
    let tmp1 = TempDir::new().unwrap();
    let tmp2 = TempDir::new().unwrap();

    let mut leaves: BTreeMap<String, Value> = BTreeMap::new();
    leaves.insert("bee/aaa".into(), json!({"hid": "aaa", "online": true, "rps": 12}));
    leaves.insert("bee/bbb".into(), json!({"hid": "bbb", "online": false}));
    leaves.insert("sid/xyz".into(), json!({"bees": ["aaa", "bbb"], "state": "idle"}));

    let t1 = TheHum::open(tmp1.path(), fresh_key(), Config::default()).unwrap();
    let t2 = TheHum::open(tmp2.path(), fresh_key(), Config::default()).unwrap();

    // Append the same logical sequence into both — they'll have different
    // signers/timestamps/prev_hashes, but the state Merkle root depends
    // only on the leaves we pass to snapshot().
    let rid = HumId::mint();
    for chi in ["hello", "prompt", "chunk", "finish"] {
        t1.append(chi, None, rid.clone(), json!({"chi": chi})).await.unwrap();
        t2.append(chi, None, rid.clone(), json!({"chi": chi})).await.unwrap();
    }

    let root1 = t1.snapshot(leaves.clone()).await.unwrap();
    let root2 = t2.snapshot(leaves.clone()).await.unwrap();

    assert_eq!(root1, root2, "snapshot root depends on leaves, not the log");
    assert_ne!(root1, [0u8; 32], "non-empty leaves yield non-zero root");
}

#[test]
fn retention_rolling_only_drops_old_files() {
    let tmp = TempDir::new().unwrap();
    let old_path = tmp.path().join("2020-01-01.ndjson");
    std::fs::write(&old_path, "").unwrap();
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let today_path = tmp.path().join(format!("{today}.ndjson"));
    std::fs::write(&today_path, "").unwrap();

    let cfg = Config {
        mode: RetentionMode::Rolling,
        days: 1,
        ..Config::default()
    };
    let t = TheHum::open(tmp.path(), fresh_key(), cfg).unwrap();
    let report = t.enforce_retention().unwrap();

    assert_eq!(report.removed_files, 1);
    assert_eq!(report.kept_files, 1);
    assert!(!old_path.exists(), "ancient file removed");
    assert!(today_path.exists(), "today file kept");
}

#[test]
fn light_mode_keeps_only_latest_daily_file() {
    let tmp = TempDir::new().unwrap();
    let days = ["2020-01-01", "2024-06-15", "2026-05-30"];
    for d in &days {
        std::fs::write(tmp.path().join(format!("{d}.ndjson")), "").unwrap();
    }

    let cfg = Config {
        mode: RetentionMode::Light,
        ..Config::default()
    };
    let t = TheHum::open(tmp.path(), fresh_key(), cfg).unwrap();
    let report = t.enforce_retention().unwrap();

    assert_eq!(report.removed_files, 2);
    assert_eq!(report.kept_files, 1);

    let remaining: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("ndjson"))
        .collect();
    assert_eq!(remaining.len(), 1, "exactly one ndjson survives");
    let stem = remaining[0].file_stem().unwrap().to_str().unwrap();
    assert_eq!(stem, "2026-05-30", "newest daily file is the survivor");
}

#[tokio::test]
async fn tail_broadcast_skips_to_lagging_receiver() {
    let tmp = TempDir::new().unwrap();
    let t = TheHum::open(tmp.path(), fresh_key(), Config::default()).unwrap();
    let mut rx = t.tail();
    let rid = HumId::mint();

    // Channel cap is 1024 — append more than that without draining.
    for i in 0..1500u32 {
        t.append("e", None, rid.clone(), json!({"i": i})).await.unwrap();
    }

    // First recv should surface a Lagged error; we just need to confirm
    // it doesn't panic and we can keep consuming.
    let mut saw_lagged = false;
    let mut received = 0usize;
    loop {
        match rx.try_recv() {
            Ok(_) => received += 1,
            Err(TryRecvError::Lagged(_)) => saw_lagged = true,
            Err(TryRecvError::Empty) | Err(TryRecvError::Closed) => break,
        }
    }
    assert!(saw_lagged, "lagging receiver must see a Lagged error");
    assert!(received > 0, "channel still yields events after lag");
}

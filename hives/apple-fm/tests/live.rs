//! The hive whole against Apple's own model: the nurse, the bridge that
//! `./build` makes, and a humd between them and a forager.
//!
//!     ./build && cargo test --test live -- --ignored
//!
//! APPLE_FM_LIVE_BRIDGE names another bridge than target/release/apple-fm-gen.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::*;
use serde_json::{json, Value};

fn bridge() -> PathBuf {
    let bridge = std::env::var("APPLE_FM_LIVE_BRIDGE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/release/apple-fm-gen"));
    assert!(bridge.is_file(), "no bridge at {}; run ./build", bridge.display());
    bridge
}

fn ask(sid: &str, model: &str, words: &str, more: Value) -> Value {
    let mut tone = json!({"chi": "prompt", "sid": sid, "modelId": model, "content": [{"kind": "text", "text": words}]});
    for (k, v) in more.as_object().unwrap() {
        tone[k] = v.clone();
    }
    tone
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs macOS 26 with Apple Intelligence on, and the bridge ./build makes"]
async fn a_conversation_grows_through_apple_s_model() {
    let nurse = Nurse::spawn("live", &bridge(), &[]);
    let since = Instant::now();
    while !nurse.log_text().contains("nurse.nestled") {
        let log = nurse.log_text();
        assert!(!log.contains("apple-fm.dormant") && !log.contains("apple-fm.never"), "Apple's model cannot run here:\n{log}");
        assert!(since.elapsed() < WAIT, "the nurse did not nestle:\n{log}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let m = nurse.model.clone();
    let mut f = Forager::link("live").await;
    f.breathed(&m).await;

    f.send(ask("talk", &m, "Reply with exactly one word: apple", json!({"temperature": 0.0})));
    let first = f.bloom("talk").await;
    assert_eq!(first.ended(), "wilt stop", "{first:?}");
    assert!(first.said.to_lowercase().contains("apple"), "{first:?}");
    assert!(first.end["nectar"]["output_tokens"].as_u64().unwrap() > 0, "{first:?}");

    f.send(ask("talk", &m, "Which fruit did you just name? Reply with that one word.", json!({"temperature": 0.0})));
    let second = f.bloom("talk").await;
    assert!(second.said.to_lowercase().contains("apple"), "the cell kept the conversation through humd: {second:?}");

    let count = "Count from 1 to 30, separated by commas, and nothing else.";
    f.send(ask("count", &m, count, json!({"temperature": 0.0})));
    let counted = f.bloom("count").await;
    assert!(counted.unfurls > 1, "the bloom streams: {counted:?}");
    assert_eq!(counted.said.matches("1, 2, 3,").count(), 1, "each unfurl is new words, not the bloom so far: {counted:?}");
    assert!(counted.said.contains("29, 30"), "{counted:?}");

    f.send(ask("frost", &m, count, json!({"temperature": 0.0, "frost": [", 10"]})));
    let frosted = f.bloom("frost").await;
    assert_eq!(frosted.ended(), "wilt stop", "{frosted:?}");
    assert!(frosted.said.trim_end().ends_with('9') && !frosted.said.contains("10"), "the bloom ends before its stop: {frosted:?}");

    f.send(ask("season", &m, "Write a long story about a bee.", json!({"temperature": 0.0, "season": 12})));
    let seasoned = f.bloom("season").await;
    assert_eq!(seasoned.ended(), "wilt max_tokens", "{seasoned:?}");
    assert_eq!(seasoned.end["nectar"]["output_tokens"], 12, "{seasoned:?}");

    f.send(ask("cancel", &m, "Write a 2000 word essay about the history of beekeeping.", json!({})));
    let first = f.on("cancel").await;
    assert_eq!(first["chi"], "unfurl", "{first}");
    let since = Instant::now();
    f.send(json!({"chi": "cancel", "sid": "cancel"}));
    assert_eq!(f.bloom("cancel").await.ended(), "wilt cancel");
    assert!(since.elapsed() < Duration::from_secs(2), "a cancel ends the bloom at once: {:?}", since.elapsed());
}

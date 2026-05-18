//! Single humd serves a prompt end-to-end via MockPerch.
//!
//! Narrative: one humd boots in-process with the mock perch. A mock
//! nestler sends a `chi:prompt` for sid "test-hum-1". The listener
//! bridge walks the canned MockPerch event sequence (system → text_delta
//! "HELLO" → result/end_turn) and broadcasts chi:chunk + chi:finish on
//! the sid. The nestler tap receives a chi:finish within 2s.
//!
//! Pure single-humd path. No ensemble routing; if this fails, the in-
//! process humd boot or the perch→listener bridge is broken.

use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn mock_prompt_yields_finish() {
    let _ = tracing_subscriber::fmt::try_init();

    let sim = sim::Sim::new();
    let a = sim.spawn_humd(ensemble::HumdId::random()).await;

    // External-perch model: humd no longer hosts perches in-process.
    // Attach a synthetic mock perch over thrum so chi:"prompt" routes.
    sim.attach_mock_perch(a.id, vec!["claude-haiku-4-5".into()])
        .await
        .expect("mock perch attaches");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    sim.nestler_send(
        a.id,
        serde_json::json!({
            "chi": "prompt",
            "rid": "p1",
            "sid": "test-hum-1",
            "modelId": "claude-haiku-4-5",
            "cwd": "/tmp",
            "content": "Say HELLO.",
        }),
    )
    .expect("nestler accepts prompt");

    // Drain tones on the sid until we see a finish (or timeout). The
    // bridge emits stream_start / text_start / text_delta / text_end /
    // content_block_stop chunks before the final finish.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut saw_text_delta = false;
    let mut finish: Option<serde_json::Value> = None;
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Some(tone) = sim.nestler_recv(a.id, "test-hum-1", remaining).await else { break };
        let chi = tone.get("chi").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if chi == "chunk" {
            if tone.get("chunkType").and_then(|v| v.as_str()) == Some("text_delta") {
                if tone.get("delta").and_then(|v| v.as_str()) == Some("HELLO") {
                    saw_text_delta = true;
                }
            }
        } else if chi == "finish" {
            finish = Some(tone);
            break;
        }
    }
    assert!(saw_text_delta, "expected a text_delta chunk with HELLO");
    let f = finish.expect("expected a finish tone within 2s");
    assert_eq!(f.get("chi").and_then(|v| v.as_str()), Some("finish"));
    assert_eq!(f.get("sid").and_then(|v| v.as_str()), Some("test-hum-1"));

    // TODO(sim-api): if the sim exposes a richer tap that returns the
    // full tone vector on the sid (e.g. `nestler_drain`), assert the
    // sequence includes a chi:chunk with chunkType:text_delta and
    // text:"HELLO" preceding the finish. For now `nestler_recv` only
    // promises the finish.
}

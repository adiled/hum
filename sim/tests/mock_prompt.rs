//! Single humd serves a prompt end-to-end via MockWorker.
//!
//! Pure single-humd path. No ensemble routing; if this fails, the in-
//! process humd boot or the worker→listener bridge is broken.

use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn mock_prompt_yields_finish() {
    let _ = tracing_subscriber::fmt::try_init();

    let sim = sim::Sim::new();
    let a = sim.spawn_humd(ensemble::Hid::random_humd()).await;

    sim.attach_mock_worker(a.id, vec!["claude-haiku-4-5".into()])
        .await
        .expect("mock worker attaches");
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

}

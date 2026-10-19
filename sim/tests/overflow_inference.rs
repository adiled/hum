//! Overflow inference.
//!
//! The prose spec is [`scenarios/overflow-inference.md`](../../../scenarios/overflow-inference.md).

use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn overflow_inference() {
    let _ = tracing_subscriber::fmt::try_init();

    let sim = sim::Sim::new();

    let humd_a = ensemble::Hid::random_humd();
    let humd_b = ensemble::Hid::random_humd();

    sim.set_capacity(humd_a, 0);

    let a = sim.spawn_humd(humd_a).await;
    let b = sim.spawn_humd(humd_b).await;

    sim.wire_unauthenticated(a.id, b.id).expect("wire humd-A ↔ humd-B");

    sim.attach_mock_worker(b.id, vec!["claude-haiku-4-5".into()])
        .await
        .expect("mock worker attaches to humd-B");

    tokio::time::sleep(Duration::from_millis(300)).await;

    sim.nestler_send(
        a.id,
        serde_json::json!({
            "chi": "prompt",
            "rid": "overflow-1",
            "sid": "overflow-1",
            "from": a.id.to_hex(),
            "modelId": "claude-haiku-4-5",
            "cwd": "/tmp",
            "content": "Say HELLO.",
        }),
    )
    .expect("humd-A nestler accepts overflow prompt");

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut saw_text_delta = false;
    let mut saw_finish = false;
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Some(tone) = sim.nestler_recv(a.id, "overflow-1", remaining).await else { break };
        let chi = tone.get("chi").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if chi == "chunk"
            && tone.get("chunkType").and_then(|v| v.as_str()) == Some("text_delta")
        {
            let text = tone
                .get("delta")
                .and_then(|v| v.get("text"))
                .and_then(|v| v.as_str())
                .or_else(|| tone.get("delta").and_then(|v| v.as_str()))
                .unwrap_or("");
            if text == "HELLO" {
                saw_text_delta = true;
            }
        }
        if chi == "finish" {
            saw_finish = true;
            break;
        }
    }

    assert!(
        saw_text_delta,
        "humd-A's nestler should see a text_delta chunk carrying HELLO routed back from humd-B"
    );
    assert!(
        saw_finish,
        "humd-A's nestler should see chi:finish within 2s of the overflow round-trip"
    );
}

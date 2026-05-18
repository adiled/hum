//! Overflow inference: a LOCAL prompt to humd-A gets offloaded to humd-B
//! because humd-A has zero spare capacity.
//!
//! Narrative: humd-A and humd-B sit in one ensemble. humd-A was spun up
//! with `capacity=0` — it's a gateway, not a kitchen. humd-B is
//! unbounded. A synthetic nestler hits humd-A with `chi:"prompt"` and
//! `sid="overflow-1"`. humd-A's daemon notices its own capacity is 0,
//! finds humd-B (advertises `claude-cli`, unbounded `free_slots`),
//! stamps the tone with `to: <humd-B>` and routes via the ensemble.
//! humd-B's MockWorkerBee produces text_delta "HELLO" + the result; the
//! reply tones flow back through the existing peer-reply forwarding
//! path and land in humd-A's nestler tap.
//!
//! This is the mirror of `phone_laptop_roam` — there, a phone nestler
//! addressed a hum on the laptop directly. Here, the nestler is naive
//! and addresses *itself* (or no one); the daemon makes the routing
//! decision.

use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn overflow_inference() {
    let _ = tracing_subscriber::fmt::try_init();

    let sim = sim::Sim::new();

    let humd_a = ensemble::Hid::random_humd();
    let humd_b = ensemble::Hid::random_humd();

    // Cap humd-A at zero BEFORE it spawns so the daemon's
    // `capacity_override` is 0 from boot. humd-B stays at default
    // (unlimited).
    sim.set_capacity(humd_a, 0);

    let a = sim.spawn_humd(humd_a).await;
    let b = sim.spawn_humd(humd_b).await;

    sim.wire(a.id, b.id).expect("wire humd-A ↔ humd-B");

    // External-worker model: B advertises a worker so A can overflow to
    // it. A has no worker (capacity 0 forces overflow anyway).
    sim.attach_mock_worker(b.id, vec!["claude-haiku-4-5".into()])
        .await
        .expect("mock worker attaches to humd-B");

    // Wait for the unsigned-hello handshake to settle so humd-A's
    // `peer_caps(humd-B)` returns the learned caps (nests, free_slots)
    // rather than the empty transport-view fallback.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Synthetic nestler on humd-A — no `to:` field (or pointed at self).
    // The prompt is "local" to humd-A; the daemon decides to overflow.
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

    // Drain the local nestler tap until we see both a text_delta of
    // "HELLO" (proves humd-B's MockWorkerBee ran AND replies traversed the
    // ensemble) and a finish (proves the wilt event made it back).
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

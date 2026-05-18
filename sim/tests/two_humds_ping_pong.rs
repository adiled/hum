//! Minimum routing proof: two humds, wired, one sends a tone targeting
//! the other's Hid, the other observes it on the ensemble inbound
//! tap.
//!
//! Narrative: humd-A and humd-B are peers in the same ensemble. From
//! humd-B's mock-nestler surface we inject a tone with
//! `to: <humd-A id hex>`. humd-B's HumdSink recognises `to:`, hands the
//! tone to its ensemble, ensemble pushes it across the wired
//! `InMemoryEndpoint::pair` link, humd-A's ensemble drains it into
//! `subscribe()`. We tap that subscription via `Sim::humd_peer_tap`.
//!
//! This test only validates the *transport seam* — no worker, no nest,
//! no prompt pipeline. If it fails, ensemble routing is broken.

use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn two_humds_ping_pong() {
    let _ = tracing_subscriber::fmt::try_init();

    let sim = sim::Sim::new();

    let a = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    let b = sim.spawn_humd(ensemble::Hid::random_humd()).await;

    sim.wire(a.id, b.id).expect("wire humd-A and humd-B");

    // Tap humd-A's inbound peer stream BEFORE sending so we don't race
    // the broadcast (broadcast::Receiver only sees tones sent after it
    // was created — earlier ones are gone). Spawn the tap in a task
    // and join later.
    let a_id = a.id;
    let sim_arc = std::sync::Arc::new(sim);
    let sim_for_tap = sim_arc.clone();
    let tap = tokio::spawn(async move {
        sim_for_tap
            .humd_peer_tap(a_id, Duration::from_secs(1))
            .await
    });

    // Tiny pause so the tap subscriber exists before B sends. Without
    // this the test races; with it the broadcast::Receiver is in place
    // before any tone is published.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // From humd-B's mock nestler, send a tone addressed to humd-A.
    // Note: `chi: "hello"` is the ensemble's peer-handshake tone and
    // gets absorbed by the drainer. Use `perf-mark` for the routing
    // test — it's a non-handshake chi that flows through.
    let tone = serde_json::json!({
        "chi": "perf-mark",
        "rid": "ping-1",
        "sid": "ping-sid",
        "to": a.id.to_hex(),
        "from": b.id.to_hex(),
        "mark": "ping",
    });
    sim_arc
        .nestler_send(b.id, tone)
        .expect("humd-B nestler accepts outbound tone");

    let got = tap.await.expect("tap task joined");

    assert!(
        got.is_some(),
        "humd-A should observe the routed tone within 1s"
    );
    let got = got.unwrap();
    assert_eq!(
        got.get("rid").and_then(|v| v.as_str()),
        Some("ping-1"),
        "rid should pass through ensemble routing unchanged"
    );
    assert_eq!(
        got.get("chi").and_then(|v| v.as_str()),
        Some("perf-mark"),
        "chi should pass through ensemble routing unchanged"
    );
}

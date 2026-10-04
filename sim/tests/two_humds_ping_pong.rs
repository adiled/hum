
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn two_humds_ping_pong() {
    let _ = tracing_subscriber::fmt::try_init();

    let sim = sim::Sim::new();

    let a = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    let b = sim.spawn_humd(ensemble::Hid::random_humd()).await;

    sim.await_ready(a.id).await.expect("a ready");
    sim.await_ready(b.id).await.expect("b ready");
    sim.wire(a.id, b.id).expect("wire humd-A and humd-B");

    let a_id = a.id;
    let sim_arc = std::sync::Arc::new(sim);
    let sim_for_tap = sim_arc.clone();
    let tap = tokio::spawn(async move {
        sim_for_tap
            .humd_peer_tap(a_id, Duration::from_secs(5))
            .await
    });

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

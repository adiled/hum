//! A worker of a kind the laptop was never taught is still a bee on the
//! server, but the laptop hands it nothing until it installs that hive.

use std::time::Duration;

async fn prompt_reaches_the_remote_worker(
    sim: &sim::Sim,
    laptop: ensemble::Hid,
    worker_rx: &mut tokio::sync::mpsc::Receiver<serde_json::Value>,
    sid: &str,
) -> bool {
    sim.nestler_send(
        laptop,
        serde_json::json!({
            "chi": "prompt",
            "rid": format!("{sid}-1"),
            "sid": sid,
            "modelId": "far-model",
            "content": "anyone of that kind out there?",
        }),
    )
    .expect("laptop nestler sends prompt");

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if let Ok(Some(tone)) =
            tokio::time::timeout(Duration::from_millis(50), worker_rx.recv()).await
            && tone.get("chi").and_then(|v| v.as_str()) == Some("prompt")
        {
            return true;
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Some(tone) = sim.nestler_recv(laptop, sid, remaining.min(Duration::from_millis(50))).await
        else {
            continue;
        };
        if tone.get("chi").and_then(|v| v.as_str()) == Some("error") {
            return false;
        }
    }
    false
}

#[tokio::test(flavor = "multi_thread")]
async fn a_remote_bee_is_routed_to_only_once_its_hive_is_installed_here() {
    let _ = tracing_subscriber::fmt::try_init();

    let sim = sim::Sim::new();
    let laptop = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    let server = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    sim.wire_unauthenticated(laptop.id, server.id).expect("L↔S");
    tokio::time::sleep(Duration::from_millis(300)).await;

    server.hives.install("far-kind");
    let worker_cid = hum_identity::HumId::mint().to_string();
    let mut worker_rx = server.thrum.register_synthetic(worker_cid.clone());
    server
        .thrum
        .inject_tone(
            &worker_cid,
            server.signed_hello(serde_json::json!({
                "chi": "hello",
                "roles": ["worker"],
                "hive": "far-kind",
                "version": "0.0.0",
                "protoVersion": thrum_core::THRUM_VERSION,
                "models": ["far-model"],
                "chis": ["hello", "prompt", "chunk", "finish"],
            })),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        server.thrum.is_connected(&worker_cid),
        "the server installed the hive, so the bee belongs there"
    );

    assert!(
        !prompt_reaches_the_remote_worker(&sim, laptop.id, &mut worker_rx, "hum-far-1").await,
        "the laptop routed to a bee whose hive it never installed"
    );

    laptop.hives.install("far-kind");
    assert!(
        prompt_reaches_the_remote_worker(&sim, laptop.id, &mut worker_rx, "hum-far-2").await,
        "once the laptop knows the kind, the remote bee is understood"
    );
}

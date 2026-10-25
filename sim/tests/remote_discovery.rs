
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn prompt_routes_to_a_nurse_found_only_by_discovery() {
    let _ = tracing_subscriber::fmt::try_init();

    let sim = sim::Sim::new();
    let laptop = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    let server = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    sim.wire_unauthenticated(laptop.id, server.id).expect("L↔S");

    tokio::time::sleep(Duration::from_millis(300)).await;

    let nurse_cid = hum_identity::HumId::mint().to_string();
    let mut nurse_rx = server.thrum.register_synthetic(nurse_cid.clone());
    let server_thrum = server.thrum.clone();
    server
        .thrum
        .inject_tone(
            &nurse_cid,
            server.signed_hello(serde_json::json!({
                "chi": "hello",
                "role": "nurse",
                "hive": "claude-cli",
                "version": "0.0.0",
                "protoVersion": thrum_core::THRUM_VERSION,
                "models": ["claude-opus-4-7"],
                "chis": ["hello", "prompt", "unfurl", "wilt"],
            })),
        )
        .await;

    let cid_for_pump = nurse_cid.clone();
    tokio::spawn(async move {
        while let Some(tone) = nurse_rx.recv().await {
            if tone.get("chi").and_then(|v| v.as_str()) == Some("prompt") {
                let sid = tone.get("sid").and_then(|v| v.as_str()).unwrap_or("").to_string();
                for reply in [
                    serde_json::json!({"chi":"unfurl","sid":&sid,"petal":0,"kind":"text"}),
                    serde_json::json!({"chi":"unfurl","sid":&sid,"petal":0,"kind":"text","text":"remote hi"}),
                    serde_json::json!({"chi":"wilt","sid":&sid,"reason":"end_turn","nectar":{}}),
                ] {
                    server_thrum.inject_tone(&cid_for_pump, reply).await;
                }
            }
        }
    });

    tokio::time::sleep(Duration::from_millis(120)).await;

    sim.nestler_send(
        laptop.id,
        serde_json::json!({
            "chi": "prompt",
            "rid": "disc-1",
            "sid": "hum-disc",
            "modelId": "claude-opus-4-7",
            "text": "who is out there?",
        }),
    )
    .expect("laptop nestler sends prompt");

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut saw_wilt = false;
    let mut saw_error = None;
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Some(tone) = sim.nestler_recv(laptop.id, "hum-disc", remaining).await else { break };
        match tone.get("chi").and_then(|v| v.as_str()) {
            Some("wilt") => {
                saw_wilt = true;
                break;
            }
            Some("error") => {
                saw_error = tone.get("message").and_then(|v| v.as_str()).map(str::to_string);
                break;
            }
            _ => {}
        }
    }

    assert!(
        saw_wilt,
        "laptop never got chi:wilt from the discovered nurse — \
         discovery did not route the prompt across the mesh{saw_error:?}",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_peer_that_joins_after_the_announce_still_finds_the_nurse() {
    let _ = tracing_subscriber::fmt::try_init();

    let sim = sim::Sim::new();
    let laptop = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    let server = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    sim.await_ready(laptop.id).await.expect("laptop ready");
    sim.await_ready(server.id).await.expect("server ready");

    let nurse_cid = hum_identity::HumId::mint().to_string();
    let mut nurse_rx = server.thrum.register_synthetic(nurse_cid.clone());
    let server_thrum = server.thrum.clone();
    server
        .thrum
        .inject_tone(
            &nurse_cid,
            server.signed_hello(serde_json::json!({
                "chi": "hello",
                "role": "nurse",
                "hive": "claude-cli",
                "version": "0.0.0",
                "protoVersion": thrum_core::THRUM_VERSION,
                "models": ["claude-opus-4-7"],
                "chis": ["hello", "prompt", "unfurl", "wilt"],
            })),
        )
        .await;

    let cid_for_pump = nurse_cid.clone();
    tokio::spawn(async move {
        while let Some(tone) = nurse_rx.recv().await {
            if tone.get("chi").and_then(|v| v.as_str()) == Some("prompt") {
                let sid = tone.get("sid").and_then(|v| v.as_str()).unwrap_or("").to_string();
                for reply in [
                    serde_json::json!({"chi":"unfurl","sid":&sid,"petal":0,"kind":"text"}),
                    serde_json::json!({"chi":"unfurl","sid":&sid,"petal":0,"kind":"text","text":"remote hi"}),
                    serde_json::json!({"chi":"wilt","sid":&sid,"reason":"end_turn","nectar":{}}),
                ] {
                    server_thrum.inject_tone(&cid_for_pump, reply).await;
                }
            }
        }
    });

    tokio::time::sleep(Duration::from_millis(120)).await;

    sim.wire_unauthenticated(laptop.id, server.id).expect("L↔S");

    tokio::time::sleep(Duration::from_millis(300)).await;

    sim.nestler_send(
        laptop.id,
        serde_json::json!({
            "chi": "prompt",
            "rid": "disc-late-1",
            "sid": "hum-disc-late",
            "modelId": "claude-opus-4-7",
            "text": "who is out there?",
        }),
    )
    .expect("laptop nestler sends prompt");

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut saw_wilt = false;
    let mut saw_error = None;
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Some(tone) = sim.nestler_recv(laptop.id, "hum-disc-late", remaining).await else { break };
        match tone.get("chi").and_then(|v| v.as_str()) {
            Some("wilt") => {
                saw_wilt = true;
                break;
            }
            Some("error") => {
                saw_error = tone.get("message").and_then(|v| v.as_str()).map(str::to_string);
                break;
            }
            _ => {}
        }
    }

    assert!(
        saw_wilt,
        "laptop never got chi:wilt from a nurse that announced before the laptop \
         joined — a peer that arrives later must still be told{saw_error:?}",
    );
}
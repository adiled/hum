use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_is_not_forwarded_on_an_unverified_capability_claim() {
    let _ = tracing_subscriber::fmt::try_init();

    let sim = sim::Sim::new();
    let laptop = sim
        .spawn_humd_not_trusting_remote_nurses(ensemble::Hid::random_humd())
        .await;
    let server = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    sim.wire_unauthenticated(laptop.id, server.id).expect("L↔S");

    tokio::time::sleep(Duration::from_millis(300)).await;

    let nurse_cid = hum_identity::HumId::mint().to_string();
    let mut claimed_rx = server.thrum.register_synthetic(nurse_cid.clone());
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

    tokio::time::sleep(Duration::from_millis(150)).await;

    sim.nestler_send(
        laptop.id,
        serde_json::json!({
            "chi": "prompt",
            "rid": "trust-1",
            "sid": "hum-trust",
            "modelId": "claude-opus-4-7",
            "text": "what is in my prompt?",
        }),
    )
    .expect("laptop nestler sends prompt");

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut forwarded = false;
    let mut refused = false;
    while std::time::Instant::now() < deadline {
        if let Ok(Some(tone)) =
            tokio::time::timeout(Duration::from_millis(50), claimed_rx.recv()).await
            && tone.get("chi").and_then(|v| v.as_str()) == Some("prompt")
        {
            forwarded = true;
            break;
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Some(tone) = sim.nestler_recv(laptop.id, "hum-trust", remaining).await else { break };
        if tone.get("chi").and_then(|v| v.as_str()) == Some("error") {
            refused = true;
            break;
        }
    }

    assert!(
        !forwarded,
        "the claiming peer received the prompt on an unverified capability claim"
    );
    assert!(refused, "laptop never got a refusal, so the prompt went nowhere");
}

#[tokio::test(flavor = "multi_thread")]
async fn opting_in_restores_discovery_routing() {
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

    tokio::time::sleep(Duration::from_millis(150)).await;

    sim.nestler_send(
        laptop.id,
        serde_json::json!({
            "chi": "prompt",
            "rid": "trust-2",
            "sid": "hum-trust",
            "modelId": "claude-opus-4-7",
            "text": "who is out there?",
        }),
    )
    .expect("laptop nestler sends prompt");

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut saw_wilt = false;
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Some(tone) = sim.nestler_recv(laptop.id, "hum-trust", remaining).await else { break };
        if tone.get("chi").and_then(|v| v.as_str()) == Some("wilt") {
            saw_wilt = true;
            break;
        }
    }

    assert!(saw_wilt, "opt-in routing did not reach the discovered nurse");
}
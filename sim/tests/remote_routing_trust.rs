use std::time::Duration;

/// A peer advertises a model it does not have. Forwarding a prompt on the
/// strength of that claim hands the prompt to whoever made the claim, so
/// humd refuses unless the operator opts in.
#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_is_not_forwarded_on_an_unverified_capability_claim() {
    let _ = tracing_subscriber::fmt::try_init();

    let sim = sim::Sim::new();
    let laptop = sim
        .spawn_humd_not_trusting_remote_workers(ensemble::Hid::random_humd())
        .await;
    let server = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    sim.wire(laptop.id, server.id).expect("L↔S");

    tokio::time::sleep(Duration::from_millis(300)).await;

    let worker_cid = hum_identity::HumId::mint().to_string();
    let mut claimed_rx = server.thrum.register_synthetic(worker_cid.clone());
    server
        .thrum
        .inject_tone(
            &worker_cid,
            serde_json::json!({
                "chi": "hello",
                "bee": ["worker"],
                "hive": "claude-cli",
                "version": "0.0.0",
                "protoVersion": thrum_core::THRUM_VERSION,
                "models": ["claude-opus-4-7"],
                "chis": ["hello", "prompt", "chunk", "finish"],
            }),
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
            "content": "what is in my prompt?",
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

/// The same mesh, with the operator opting in, still routes. Proves the
/// interlock is the only thing standing in the way.
#[tokio::test(flavor = "multi_thread")]
async fn opting_in_restores_discovery_routing() {
    let _ = tracing_subscriber::fmt::try_init();

    let sim = sim::Sim::new();
    let laptop = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    let server = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    sim.wire(laptop.id, server.id).expect("L↔S");

    tokio::time::sleep(Duration::from_millis(300)).await;

    let worker_cid = hum_identity::HumId::mint().to_string();
    let mut worker_rx = server.thrum.register_synthetic(worker_cid.clone());
    let server_thrum = server.thrum.clone();
    server
        .thrum
        .inject_tone(
            &worker_cid,
            serde_json::json!({
                "chi": "hello",
                "bee": ["worker"],
                "hive": "claude-cli",
                "version": "0.0.0",
                "protoVersion": thrum_core::THRUM_VERSION,
                "models": ["claude-opus-4-7"],
                "chis": ["hello", "prompt", "chunk", "finish"],
            }),
        )
        .await;

    let cid_for_pump = worker_cid.clone();
    tokio::spawn(async move {
        while let Some(tone) = worker_rx.recv().await {
            if tone.get("chi").and_then(|v| v.as_str()) == Some("prompt") {
                let sid = tone.get("sid").and_then(|v| v.as_str()).unwrap_or("").to_string();
                for reply in [
                    serde_json::json!({"chi":"chunk","sid":&sid,"chunkType":"text_start","id":0}),
                    serde_json::json!({"chi":"chunk","sid":&sid,"chunkType":"text_delta","delta":"remote hi"}),
                    serde_json::json!({"chi":"finish","sid":&sid,"finishReason":"end_turn","usage":{}}),
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
            "content": "who is out there?",
        }),
    )
    .expect("laptop nestler sends prompt");

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut saw_finish = false;
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Some(tone) = sim.nestler_recv(laptop.id, "hum-trust", remaining).await else { break };
        if tone.get("chi").and_then(|v| v.as_str()) == Some("finish") {
            saw_finish = true;
            break;
        }
    }

    assert!(saw_finish, "opt-in routing did not reach the discovered worker");
}
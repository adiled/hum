
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn prompt_routes_to_a_worker_found_only_by_discovery() {
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

    tokio::time::sleep(Duration::from_millis(120)).await;

    sim.nestler_send(
        laptop.id,
        serde_json::json!({
            "chi": "prompt",
            "rid": "disc-1",
            "sid": "hum-disc",
            "modelId": "claude-opus-4-7",
            "content": "who is out there?",
        }),
    )
    .expect("laptop nestler sends prompt");

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut saw_finish = false;
    let mut saw_error = None;
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Some(tone) = sim.nestler_recv(laptop.id, "hum-disc", remaining).await else { break };
        match tone.get("chi").and_then(|v| v.as_str()) {
            Some("finish") => {
                saw_finish = true;
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
        saw_finish,
        "laptop never got chi:finish from the discovered worker — \
         discovery did not route the prompt across the mesh{saw_error:?}",
    );
}
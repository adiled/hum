//! eggs-on-the-hum — the role-distributed bloom.
//!
//! The prose spec is [`scenarios/eggs-on-the-hum.md`](../../../scenarios/eggs-on-the-hum.md).

use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn eggs_on_the_hum() {
    let _ = tracing_subscriber::fmt::try_init();

    let sim = sim::Sim::new();

    let laptop      = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    let server      = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    let workstation = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    let phone       = sim.spawn_humd(ensemble::Hid::random_humd()).await;

    sim.wire_unauthenticated(laptop.id, server.id).expect("L↔S");
    sim.wire_unauthenticated(laptop.id, workstation.id).expect("L↔W");
    sim.wire_unauthenticated(laptop.id, phone.id).expect("L↔P");
    sim.wire_unauthenticated(server.id, workstation.id).expect("S↔W");
    sim.wire_unauthenticated(server.id, phone.id).expect("S↔P");
    sim.wire_unauthenticated(workstation.id, phone.id).expect("W↔P");

    sim.attach_mock_tools(
        workstation.id,
        "humfs",
        vec!["humfs_read".into(), "humfs_do_code".into()],
        |name, _args| match name {
            "humfs_read" => "[mock humfs on workstation] auth.ts contents".to_string(),
            "humfs_do_code" => "Edited (1 occurrence replaced)".to_string(),
            other => format!("(unhandled tool {other})"),
        },
    )
    .await
    .expect("mock humfs attaches to workstation");

    let server_thrum = server.thrum.clone();
    let worker_cid = hum_identity::HumId::mint().to_string();
    let mut worker_rx = server.thrum.register_synthetic(worker_cid.clone());
    let worker_hello = serde_json::json!({
        "chi": "hello",
        "roles": ["worker"],
        "hive": "claude-cli",
        "version": "0.0.0",
        "protoVersion": thrum_core::THRUM_VERSION,
        "models": ["claude-opus-4-7"],
        "chis": ["hello", "prompt", "unfurl", "tendril-reach", "wilt"],
    });
    server.thrum.inject_tone(&worker_cid, server.signed_hello(worker_hello)).await;

    let worker_cid_for_pump = worker_cid.clone();
    let server_thrum_for_pump = server_thrum.clone();
    tokio::spawn(async move {
        while let Some(tone) = worker_rx.recv().await {
            let chi = tone.get("chi").and_then(|v| v.as_str()).unwrap_or("");
            match chi {
                "prompt" => {
                    let sid = tone.get("sid").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    for tone in [
                        serde_json::json!({"chi":"unfurl","sid":&sid,"petal":0,"kind":"text"}),
                        serde_json::json!({"chi":"unfurl","sid":&sid,"petal":0,"kind":"text","text":"reading file…"}),
                        serde_json::json!({
                            "chi":"tendril-reach","sid":&sid,
                            "callId":"call-eggs-1",
                            "name":"humfs_read",
                            "args":{"file_path":"/work/auth.ts"},
                        }),
                    ] {
                        server_thrum_for_pump.inject_tone(&worker_cid_for_pump, tone).await;
                    }
                }
                "tendril-result" => {
                    let sid = tone.get("sid").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    for tone in [
                        serde_json::json!({"chi":"unfurl","sid":&sid,"petal":0,"kind":"text","text":"…done."}),
                        serde_json::json!({"chi":"unfurl","sid":&sid,"petal":0,"kind":"text","closed":true}),
                        serde_json::json!({"chi":"wilt","sid":&sid,"reason":"end_turn","nectar":{}}),
                    ] {
                        server_thrum_for_pump.inject_tone(&worker_cid_for_pump, tone).await;
                    }
                }
                _ => {}
            }
        }
    });

    let _phone_cid = sim
        .attach_observer(phone.id, server.id, "hum-eggs")
        .expect("phone observer attach");
    tokio::time::sleep(Duration::from_millis(50)).await;

    let cwd_uri = format!("hum://{}/work", workstation.id.to_hex());
    sim.nestler_send(
        laptop.id,
        serde_json::json!({
            "chi": "prompt",
            "rid": "eggs-1",
            "sid": "hum-eggs",
            "to": server.id.to_hex(),
            "from": laptop.id.to_hex(),
            "modelId": "claude-opus-4-7",
            "cwd": cwd_uri,
            "text": "Refactor auth middleware.",
        }),
    )
    .expect("laptop nestler sends prompt");

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut saw_wilt = false;
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Some(tone) = sim.nestler_recv(phone.id, "hum-eggs", remaining).await else { break };
        let chi = tone.get("chi").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if chi == "wilt" {
            saw_wilt = true;
            break;
        }
    }

    assert!(
        saw_wilt,
        "phone (SMS-relay) never observed chi:wilt for hum-eggs across the 4-humd mesh — \
         the cross-humd tendril-reach → workstation humfs → tendril-result return path \
         didn't close."
    );
}

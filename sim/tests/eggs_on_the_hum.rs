//! eggs-on-the-hum — the role-distributed bloom.
//!
//! Operator on laptop fires one prompt, then walks away. Four
//! humds, four roles:
//!
//! - **laptop** (humd-L): nestler — sends `chi:"prompt"`
//! - **server** (humd-S): worker bee — runs the LLM
//! - **workstation** (humd-W): humfs forager — handles tool-calls
//! - **phone** (humd-P): SMS-relay observer — sees `chi:"finish"`
//!
//! Coverage:
//! - laptop pins fs to workstation via `cwd: "hum://humd_<W>/work"`
//! - humd-S parses the URI, stashes `(sid → workstation hid)`, and
//!   routes the worker's chi:"tool-call" via ensemble to humd-W
//! - humd-W routes inbound tool-call to its local humfs forager
//! - humfs's chi:"tool-result" routes back via ensemble to humd-S
//! - humd-S forwards the result to the worker
//! - worker emits finish; observer fan-out reaches phone

use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn eggs_on_the_hum() {
    let _ = tracing_subscriber::fmt::try_init();

    let sim = sim::Sim::new();

    let laptop      = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    let server      = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    let workstation = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    let phone       = sim.spawn_humd(ensemble::Hid::random_humd()).await;

    // Full mesh — simpler than relaying for the test.
    sim.wire(laptop.id, server.id).expect("L↔S");
    sim.wire(laptop.id, workstation.id).expect("L↔W");
    sim.wire(laptop.id, phone.id).expect("L↔P");
    sim.wire(server.id, workstation.id).expect("S↔W");
    sim.wire(server.id, phone.id).expect("S↔P");
    sim.wire(workstation.id, phone.id).expect("W↔P");

    // Mock humfs forager on the WORKSTATION (not the server).
    // Tool-calls reach it only via cross-humd routing (Q5).
    sim.attach_mock_forager(
        workstation.id,
        "humfs",
        vec!["humfs_read".into(), "humfs_do_code".into()],
        |tool_name, _args| match tool_name {
            "humfs_read" => "[mock humfs on workstation] auth.ts contents".to_string(),
            "humfs_do_code" => "Edited (1 occurrence replaced)".to_string(),
            other => format!("(unhandled tool {other})"),
        },
    )
    .await
    .expect("humfs forager attaches to workstation");

    // Mock worker on the SERVER. Emits one chi:tool-call after
    // receiving the prompt, then chi:finish after the tool-result.
    let server_thrum = server.thrum.clone();
    let worker_cid = format!("sim-worker-{}", uuid::Uuid::new_v4());
    let mut worker_rx = server.thrum.register_synthetic(worker_cid.clone());
    let worker_hello = serde_json::json!({
        "chi": "hello",
        "bee": ["worker"],
        "hive": "claude-cli",
        "version": "0.0.0",
        "protoVersion": thrum_core::THRUM_VERSION,
        "models": ["claude-opus-4-7"],
        "chis": ["hello", "prompt", "chunk", "tool-call", "finish"],
    });
    server.thrum.inject_tone(&worker_cid, worker_hello).await;

    let worker_cid_for_pump = worker_cid.clone();
    let server_thrum_for_pump = server_thrum.clone();
    tokio::spawn(async move {
        while let Some(tone) = worker_rx.recv().await {
            let chi = tone.get("chi").and_then(|v| v.as_str()).unwrap_or("");
            match chi {
                "prompt" => {
                    let sid = tone.get("sid").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    for tone in [
                        serde_json::json!({"chi":"chunk","sid":&sid,"chunkType":"text_start","id":0}),
                        serde_json::json!({"chi":"chunk","sid":&sid,"chunkType":"text_delta","delta":"reading file…"}),
                        serde_json::json!({
                            "chi":"tool-call","sid":&sid,
                            "callId":"call-eggs-1",
                            "toolName":"humfs_read",
                            "name":"humfs_read",
                            "args":{"file_path":"/work/auth.ts"},
                        }),
                    ] {
                        server_thrum_for_pump.inject_tone(&worker_cid_for_pump, tone).await;
                    }
                }
                "tool-result" => {
                    let sid = tone.get("sid").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    for tone in [
                        serde_json::json!({"chi":"chunk","sid":&sid,"chunkType":"text_delta","delta":"…done."}),
                        serde_json::json!({"chi":"chunk","sid":&sid,"chunkType":"content_block_stop","blockIdx":0}),
                        serde_json::json!({"chi":"finish","sid":&sid,"finishReason":"end_turn","usage":{}}),
                    ] {
                        server_thrum_for_pump.inject_tone(&worker_cid_for_pump, tone).await;
                    }
                }
                _ => {}
            }
        }
    });

    // Phone attaches as a hearOnly observer.
    let _phone_cid = sim
        .attach_observer(phone.id, server.id, "hum-eggs")
        .expect("phone observer attach");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Laptop fires the prompt. The cwd pins the workstation as the
    // fs host using the workstation's full hid in `hum://` form.
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
            "content": "Refactor auth middleware.",
        }),
    )
    .expect("laptop nestler sends prompt");

    // Drain the phone's mailbox until finish lands.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut saw_finish = false;
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Some(tone) = sim.nestler_recv(phone.id, "hum-eggs", remaining).await else { break };
        let chi = tone.get("chi").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if chi == "finish" {
            saw_finish = true;
            break;
        }
    }

    assert!(
        saw_finish,
        "phone (SMS-relay) never observed chi:finish for hum-eggs across the 4-humd mesh — \
         the cross-humd tool-call → workstation humfs → tool-result return path \
         didn't close."
    );
}

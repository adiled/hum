//! eggs-on-the-hum — the role-distributed bloom.
//!
//! Operator on laptop fires one prompt, then walks away. The worker
//! lives on a different humd. The fs lives on a different humd
//! still. The phone, on yet another humd, observes the bloom in
//! hearOnly mode and is supposed to see the `chi:"finish"` so its
//! twilio-sms forager can SMS the operator at the egg counter.
//!
//! Scenario doc: `scenarios/eggs-on-the-hum.md`.
//!
//! Coverage:
//! - Worker on humd-S handles a `chi:"prompt"` from laptop and
//!   emits a `chi:"tool-call"` mid-bloom for a forager-advertised
//!   tool (humfs_read).
//! - humd-S's tool-call router (P8) looks up its LOCAL forager
//!   manifests for a hive carrying that toolName and routes the
//!   tone there. Test uses an in-process mock humfs forager
//!   attached on humd-S to exercise this seam.
//! - Phone humd attaches as a hearOnly observer of the hum's sid.
//!   humd-S's worker-passthrough fan-out (the observer roster)
//!   stamps each reply tone with `to: <phone.humd>` and routes
//!   through the ensemble.
//! - The final `chi:"finish"` reaches the phone's mailbox.
//!
//! What's NOT exercised here (deferred to P11, cross-humd forager-
//! tool routing): humfs running on a DIFFERENT humd than the worker.
//! humd-S today only consults its own manifest registry for the
//! forager lookup. The 4-device narrative in the scenario doc
//! collapses to 3 humds in this test (humfs sits with the worker)
//! until the cross-humd forager routing lands.

use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn eggs_on_the_hum() {
    let _ = tracing_subscriber::fmt::try_init();

    let sim = sim::Sim::new();

    let laptop = sim.spawn_humd(ensemble::HumdId::random()).await;
    let server = sim.spawn_humd(ensemble::HumdId::random()).await;
    let phone  = sim.spawn_humd(ensemble::HumdId::random()).await;

    sim.wire(laptop.id, server.id).expect("laptop ↔ server");
    sim.wire(server.id, phone.id).expect("server ↔ phone");
    sim.wire(laptop.id, phone.id).expect("laptop ↔ phone");

    // Mock humfs forager attached at the SERVER humd (colocated with
    // the worker until P11 lifts forager routing to ensemble).
    sim.attach_mock_forager(
        server.id,
        "humfs",
        vec!["humfs_read".into(), "humfs_do_code".into()],
        |tool_name, _args| match tool_name {
            "humfs_read" => "[mock humfs] file content: \"auth middleware\"".to_string(),
            "humfs_do_code" => "Edited (1 occurrence replaced)".to_string(),
            other => format!("(unhandled tool {other})"),
        },
    )
    .await
    .expect("humfs forager attaches to server");

    // Mock worker that emits ONE chi:tool-call mid-bloom and then a
    // chi:finish. The simulated worker registers as bee:["worker"]
    // advertising the model the prompt will ask for.
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
        // The worker treats inbound tones two ways:
        //   prompt → emit one tool-call, wait for its tool-result, then finish
        //   tool-result → continue (emit final chunks + finish)
        while let Some(tone) = worker_rx.recv().await {
            let chi = tone.get("chi").and_then(|v| v.as_str()).unwrap_or("");
            match chi {
                "prompt" => {
                    let sid = tone.get("sid").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    // Initial text + tool-call request.
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
                        serde_json::json!({"chi":"chunk","sid":&sid,"chunkType":"text_delta","delta":"…done refactor."}),
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

    // Phone attaches as a hearOnly observer of the hum sid on the
    // server. Worker-passthrough fan-out will stamp each reply tone
    // with `to: <phone>` and route via the ensemble.
    let _phone_cid = sim
        .attach_observer(phone.id, server.id, "hum-eggs")
        .expect("phone observer attach");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Laptop fires the prompt addressed at the server humd.
    sim.nestler_send(
        laptop.id,
        serde_json::json!({
            "chi": "prompt",
            "rid": "eggs-1",
            "sid": "hum-eggs",
            "to": server.id.to_hex(),
            "from": laptop.id.to_hex(),
            "modelId": "claude-opus-4-7",
            "cwd": "/work",
            "content": "Refactor auth middleware.",
        }),
    )
    .expect("laptop nestler sends prompt");

    // Drain the phone's mailbox until finish lands or the deadline
    // expires.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
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
        "phone (the SMS-relay humd) never observed chi:finish for hum-eggs — \
         either the worker's tool-call didn't route to humfs, the tool-result \
         didn't return to the worker, or the observer fan-out didn't reach \
         the phone."
    );
}

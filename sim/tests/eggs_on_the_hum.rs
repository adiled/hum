//! eggs-on-the-hum — the role-distributed bloom.
//!
//! The prose spec is [`scenarios/eggs-on-the-hum.md`](../../../scenarios/eggs-on-the-hum.md).
//! A forager on the laptop opens a bloom that builds on the workstation;
//! the nurse that grows it is on the server; the phone watches. The
//! nurse's `bash` crosses to the workstation's builder and comes back.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;

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

    let ran: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let ran_on_workstation = ran.clone();
    sim.attach_mock_builder(workstation.id, move |cwd, command| {
        ran_on_workstation.lock().push((cwd.to_string(), command.to_string()));
        ("export function auth() {}".to_string(), 0)
    })
    .await
    .expect("a builder lends the workstation");

    let server_thrum = server.thrum.clone();
    let nurse_cid = hum_identity::HumId::mint().to_string();
    let mut nurse_rx = server.thrum.register_synthetic(nurse_cid.clone());
    let nurse_hello = serde_json::json!({
        "chi": "hello",
        "role": "nurse",
        "hive": "claude-cli",
        "version": "0.0.0",
        "protoVersion": thrum_core::THRUM_VERSION,
        "models": ["claude-opus-4-7"],
        "chis": ["hello", "prompt", "unfurl", "tendril-reach", "wilt"],
    });
    server.thrum.inject_tone(&nurse_cid, server.signed_hello(nurse_hello)).await;

    let grasped: Arc<Mutex<Option<serde_json::Value>>> = Arc::default();
    let grasped_by_nurse = grasped.clone();
    let nurse_cid_for_pump = nurse_cid.clone();
    let server_thrum_for_pump = server_thrum.clone();
    tokio::spawn(async move {
        while let Some(tone) = nurse_rx.recv().await {
            let chi = tone.get("chi").and_then(|v| v.as_str()).unwrap_or("");
            let sid = tone.get("sid").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let replies = match chi {
                "prompt" => vec![
                    serde_json::json!({"chi":"unfurl","sid":&sid,"petal":0,"kind":"text"}),
                    serde_json::json!({"chi":"unfurl","sid":&sid,"petal":0,"kind":"text","text":"reading file…"}),
                    serde_json::json!({
                        "chi":"tendril-reach","sid":&sid,
                        "callId":"call-eggs-1",
                        "name":"bash",
                        "args":{"command":"cat auth.ts"},
                    }),
                ],
                "tendril-result" => {
                    *grasped_by_nurse.lock() = Some(tone.clone());
                    vec![
                        serde_json::json!({"chi":"unfurl","sid":&sid,"petal":0,"kind":"text","text":"…done."}),
                        serde_json::json!({"chi":"unfurl","sid":&sid,"petal":0,"kind":"text","closed":true}),
                        serde_json::json!({"chi":"wilt","sid":&sid,"reason":"end_turn","nectar":{}}),
                    ]
                }
                _ => vec![],
            };
            for tone in replies {
                server_thrum_for_pump.inject_tone(&nurse_cid_for_pump, tone).await;
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
        if tone.get("chi").and_then(|v| v.as_str()) == Some("wilt") {
            saw_wilt = true;
            break;
        }
    }

    assert!(
        saw_wilt,
        "phone (SMS-relay) never observed chi:wilt for hum-eggs across the 4-humd mesh — \
         the cross-humd bash reach → workstation builder → tendril-result return path \
         didn't close."
    );
    assert_eq!(
        ran.lock().as_slice(),
        &[("/work".to_string(), "cat auth.ts".to_string())],
        "the workstation's builder ran the command where the bloom builds"
    );
    let grasped = grasped.lock().clone().expect("the nurse heard what bash grasped");
    assert_eq!(grasped["output"], "export function auth() {}");
    assert_eq!(grasped["isError"], false);
}

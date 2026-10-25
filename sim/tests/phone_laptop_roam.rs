//! Roam.
//!
//! The prose spec is [`scenarios/phone-laptop-roam.md`](../../../scenarios/phone-laptop-roam.md).

use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn phone_laptop_roam() {
    let _ = tracing_subscriber::fmt::try_init();

    let sim = sim::Sim::new();

    let laptop = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    let phone = sim.spawn_humd(ensemble::Hid::random_humd()).await;

    sim.wire_unauthenticated(laptop.id, phone.id).expect("wire laptop ↔ phone");

    sim.attach_mock_worker(laptop.id, vec!["claude-haiku-4-5".into()])
        .await
        .expect("mock worker attaches to laptop");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    sim.nestler_send(
        phone.id,
        serde_json::json!({
            "chi": "prompt",
            "rid": "roam-1",
            "sid": "hum-X",
            "to": laptop.id.to_hex(),
            "from": phone.id.to_hex(),
            "modelId": "claude-haiku-4-5",
            "cwd": "/tmp",
            "text": "Say HELLO.",
        }),
    )
    .expect("phone nestler accepts outbound roam prompt");

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut saw_text = false;
    let mut wilt: Option<serde_json::Value> = None;
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Some(tone) = sim.nestler_recv(phone.id, "hum-X", remaining).await else { break };
        let chi = tone.get("chi").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if chi == "unfurl"
            && tone.get("kind").and_then(|v| v.as_str()) == Some("text")
            && tone.get("text").and_then(|v| v.as_str()) == Some("HELLO")
        {
            saw_text = true;
        }
        if chi == "wilt" {
            wilt = Some(tone);
            break;
        }
    }

    assert!(saw_text, "phone should see an unfurl of HELLO routed back");
    let f = wilt.expect("phone should receive chi:wilt for hum-X within 2s");
    assert_eq!(f.get("chi").and_then(|v| v.as_str()), Some("wilt"));
    assert_eq!(f.get("sid").and_then(|v| v.as_str()), Some("hum-X"));
    assert_eq!(
        f.get("to").and_then(|v| v.as_str()),
        Some(phone.id.to_hex().as_str()),
        "wilt tone should be addressed at the originating humd"
    );
}

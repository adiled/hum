//! Roam: phone hums against a hum hosted on the laptop.
//!
//! Narrative: two humds — `laptop` and `phone` — wired into one
//! ensemble. Each knows the other as a peer. The user is on their
//! phone but the active hum ("hum-X") lives on the laptop. Phone's
//! mock nestler sends a chi:prompt addressed at the laptop's HumdId.
//!
//! Expected flow:
//!   1. phone humd's ToneSink sees `to: laptop` → ensemble route → wire
//!   2. laptop humd's HumdSink receives the prompt
//!   3. laptop's MockWorkerBee produces text_delta "HELLO" + result
//!   4. listener bridge fires chi:chunk + chi:finish for sid "hum-X"
//!   5. laptop's HumdSink replies *back to phone* (because the prompt
//!      came from there) — chi:finish flows phone-ward over the wire
//!   6. phone's nestler tap receives chi:finish within 2s
//!
//! Step 5 is the load-bearing piece that the other agents may not have
//! built yet: the reply path (chunk + finish addressed at the origin
//! humd) requires the daemon to remember *where* a prompt came from
//! and route reply tones back. If the ensemble has only unidirectional
//! routing today, this test will fail at the recv step — hence
//! `#[ignore]` is opt-in.
//!
//! Remove `#[ignore]` once `feat(ensemble): reply-tone routing` lands.

use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn phone_laptop_roam() {
    let _ = tracing_subscriber::fmt::try_init();

    let sim = sim::Sim::new();

    let laptop = sim.spawn_humd(ensemble::HumdId::random()).await;
    let phone = sim.spawn_humd(ensemble::HumdId::random()).await;

    sim.wire(laptop.id, phone.id).expect("wire laptop ↔ phone");

    // External-perch model: laptop hosts the perch; phone is just an
    // access surface that routes via `to:`.
    sim.attach_mock_worker(laptop.id, vec!["claude-haiku-4-5".into()])
        .await
        .expect("mock perch attaches to laptop");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Phone's nestler sends a prompt addressed at the laptop. The hum
    // "hum-X" is hosted on the laptop; the phone is the access surface.
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
            "content": "Say HELLO.",
        }),
    )
    .expect("phone nestler accepts outbound roam prompt");

    // Phone's nestler tap drains tones on hum-X until it sees the
    // finish. The bridge emits chunks (stream_start / text_delta /
    // content_block_stop) before the finish — they all stamp with
    // `to: <phone.id>` and route back across the wire.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut saw_text_delta = false;
    let mut finish: Option<serde_json::Value> = None;
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Some(tone) = sim.nestler_recv(phone.id, "hum-X", remaining).await else { break };
        let chi = tone.get("chi").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if chi == "chunk"
            && tone.get("chunkType").and_then(|v| v.as_str()) == Some("text_delta")
            && tone.get("delta").and_then(|v| v.as_str()) == Some("HELLO")
        {
            saw_text_delta = true;
        }
        if chi == "finish" {
            finish = Some(tone);
            break;
        }
    }

    assert!(saw_text_delta, "phone should see a text_delta chunk with HELLO routed back");
    let f = finish.expect("phone should receive chi:finish for hum-X within 2s");
    assert_eq!(f.get("chi").and_then(|v| v.as_str()), Some("finish"));
    assert_eq!(f.get("sid").and_then(|v| v.as_str()), Some("hum-X"));
    // The reply tone carries `to: phone.id` because the laptop addressed
    // it back at the prompt's origin.
    assert_eq!(
        f.get("to").and_then(|v| v.as_str()),
        Some(phone.id.to_hex().as_str()),
        "finish tone should be addressed at the originating humd"
    );
}

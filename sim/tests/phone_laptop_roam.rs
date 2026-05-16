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
//!   3. laptop's MockPerch produces text_delta "HELLO" + result
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
#[ignore = "requires bidirectional ensemble routing for chunk/finish reply tones"]
async fn phone_laptop_roam() {
    let _ = tracing_subscriber::fmt::try_init();

    let sim = sim::Sim::new();

    let laptop = sim.spawn_humd(ensemble::HumdId::random()).await;
    let phone = sim.spawn_humd(ensemble::HumdId::random()).await;

    sim.wire(laptop.id, phone.id).expect("wire laptop ↔ phone");

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

    // Phone's nestler tap should receive a chi:finish once the laptop's
    // MockPerch has walked the canned event sequence and the reply has
    // travelled back across the wire.
    //
    // Generous timeout — two humds, one hop, one mock turn.
    let finish = sim
        .nestler_recv(phone.id, "hum-X", Duration::from_secs(2))
        .await;

    assert!(
        finish.is_some(),
        "phone should receive chi:finish for hum-X within 2s — \
         reply-tone routing missing if this is None"
    );
    let f = finish.unwrap();
    assert_eq!(
        f.get("chi").and_then(|v| v.as_str()),
        Some("finish"),
        "phone tap should see chi:finish, got {f:?}"
    );
    assert_eq!(
        f.get("sid").and_then(|v| v.as_str()),
        Some("hum-X"),
        "finish must carry sid hum-X across the wire"
    );

    // TODO(sim-api): the laptop's HumdSink needs to mark the prompt's
    // origin (humd-B) on the listener so the wilt/petal callbacks know
    // to address reply tones with `to: <phone.id hex>`. If sim exposes
    // an explicit hook (e.g. `sim.set_reply_target(laptop.id, sid,
    // phone.id)`), wire it here once available.
}

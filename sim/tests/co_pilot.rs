//! Co-pilot: two operators, one hum — one drives, one observes.
//!
//! Narrative: `laptop` hosts hum `hum-X`. The driver nestler (also on
//! laptop) sends a prompt and expects the bloom. A second nestler on
//! `phone`'s humd attaches in `hearOnly` mode: it tells the phone humd
//! "I want to listen to hum-X over on laptop." The phone humd forwards
//! a `chi:"attach"` tone to laptop, which records phone as an observer.
//!
//! Expected flow:
//!   1. laptop's driver nestler sends chi:prompt, sid="hum-X", local.
//!   2. phone's observer nestler sends chi:attach, sid="hum-X",
//!      to: laptop, hearOnly: true.
//!   3. laptop's HumdSink records {sid="hum-X", observer=phone}.
//!   4. MockWorkerBee fires text_delta "HELLO" + finish.
//!   5. NestListener emits reply tones; each is:
//!        a) broadcast locally → driver sees it
//!        b) fan-out routed `to: phone` over the ensemble → observer sees it
//!   6. Both nestler taps see text_delta "HELLO" + finish for hum-X.

use std::time::Duration;

#[derive(Default, Debug)]
struct TapTranscript {
    saw_text_delta: bool,
    finish: Option<serde_json::Value>,
}

/// Drain `nestler_recv` until a finish lands or the deadline elapses.
/// Records whether a text_delta "HELLO" appeared along the way.
async fn drain_until_finish(
    sim: &sim::Sim,
    humd: ensemble::HumdId,
    sid: &str,
    deadline: std::time::Instant,
) -> TapTranscript {
    let mut t = TapTranscript::default();
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Some(tone) = sim.nestler_recv(humd, sid, remaining).await else { break };
        let chi = tone.get("chi").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if chi == "chunk"
            && tone.get("chunkType").and_then(|v| v.as_str()) == Some("text_delta")
            && tone.get("delta").and_then(|v| v.as_str()) == Some("HELLO")
        {
            t.saw_text_delta = true;
        }
        if chi == "finish" {
            t.finish = Some(tone);
            break;
        }
    }
    t
}

#[tokio::test(flavor = "multi_thread")]
async fn co_pilot_fanout() {
    let _ = tracing_subscriber::fmt::try_init();

    let sim = sim::Sim::new();
    let laptop = sim.spawn_humd(ensemble::HumdId::random()).await;
    let phone = sim.spawn_humd(ensemble::HumdId::random()).await;
    sim.wire(laptop.id, phone.id).expect("wire laptop ↔ phone");

    // External-worker model: humd's a router. Attach a synthetic mock
    // worker to laptop so the prompt can be served there.
    sim.attach_mock_worker(laptop.id, vec!["claude-haiku-4-5".into()])
        .await
        .expect("mock worker attaches to laptop");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // 1) Observer on phone announces interest in hum-X over on laptop.
    //    Done first so it registers before any reply tones arrive.
    sim.attach_observer(phone.id, laptop.id, "hum-X")
        .expect("phone attaches as hearOnly observer");

    // Give the attach tone a beat to traverse the ensemble pump before
    // the prompt's reply tones start flowing — without this, the prompt
    // can finish faster than the observer registration lands and the
    // fan-out roster is empty when MockWorkerBee emits its delta.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // 2) Driver on laptop sends the actual prompt. No `to:` field — the
    //    prompt is local to laptop, dispatched straight into the nest.
    sim.nestler_send(
        laptop.id,
        serde_json::json!({
            "chi": "prompt",
            "rid": "drive-1",
            "sid": "hum-X",
            "modelId": "claude-haiku-4-5",
            "cwd": "/tmp",
            "content": "Say HELLO.",
        }),
    )
    .expect("laptop nestler accepts local prompt");

    // 3) Both nestler taps drain in parallel.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let (driver_tape, observer_tape) = tokio::join!(
        drain_until_finish(&sim, laptop.id, "hum-X", deadline),
        drain_until_finish(&sim, phone.id, "hum-X", deadline),
    );

    // Driver tap — local nestler on laptop, no cross-humd hop.
    assert!(driver_tape.saw_text_delta, "driver should see text_delta HELLO");
    let df = driver_tape.finish.expect("driver should see chi:finish for hum-X");
    assert_eq!(df.get("chi").and_then(|v| v.as_str()), Some("finish"));
    assert_eq!(df.get("sid").and_then(|v| v.as_str()), Some("hum-X"));

    // Observer tap — phone, the second humd, never sent the prompt.
    assert!(observer_tape.saw_text_delta, "observer should see text_delta HELLO");
    let of = observer_tape.finish.expect("observer should see chi:finish for hum-X");
    assert_eq!(of.get("chi").and_then(|v| v.as_str()), Some("finish"));
    assert_eq!(of.get("sid").and_then(|v| v.as_str()), Some("hum-X"));
    // The fan-out copy is addressed at the phone, stamped by the laptop.
    assert_eq!(
        of.get("to").and_then(|v| v.as_str()),
        Some(phone.id.to_hex().as_str()),
        "observer's finish tone must carry to: phone — proves the fan-out path stamped it"
    );
}

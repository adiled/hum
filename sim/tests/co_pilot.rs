//! Co-pilot.
//!
//! The prose spec is [`scenarios/co-pilot.md`](../../../scenarios/co-pilot.md).

use std::time::Duration;

#[derive(Default, Debug)]
struct TapTranscript {
    saw_text_delta: bool,
    finish: Option<serde_json::Value>,
}

async fn drain_until_finish(
    sim: &sim::Sim,
    humd: ensemble::Hid,
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
    let laptop = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    let phone = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    sim.wire_unauthenticated(laptop.id, phone.id).expect("wire laptop ↔ phone");

    sim.attach_mock_worker(laptop.id, vec!["claude-haiku-4-5".into()])
        .await
        .expect("mock worker attaches to laptop");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    sim.attach_observer(phone.id, laptop.id, "hum-X")
        .expect("phone attaches as hearOnly observer");

    tokio::time::sleep(Duration::from_millis(50)).await;

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

    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let (driver_tape, observer_tape) = tokio::join!(
        drain_until_finish(&sim, laptop.id, "hum-X", deadline),
        drain_until_finish(&sim, phone.id, "hum-X", deadline),
    );

    assert!(driver_tape.saw_text_delta, "driver should see text_delta HELLO");
    let df = driver_tape.finish.expect("driver should see chi:finish for hum-X");
    assert_eq!(df.get("chi").and_then(|v| v.as_str()), Some("finish"));
    assert_eq!(df.get("sid").and_then(|v| v.as_str()), Some("hum-X"));

    assert!(observer_tape.saw_text_delta, "observer should see text_delta HELLO");
    let of = observer_tape.finish.expect("observer should see chi:finish for hum-X");
    assert_eq!(of.get("chi").and_then(|v| v.as_str()), Some("finish"));
    assert_eq!(of.get("sid").and_then(|v| v.as_str()), Some("hum-X"));
    assert_eq!(
        of.get("to").and_then(|v| v.as_str()),
        Some(phone.id.to_hex().as_str()),
        "observer's finish tone must carry to: phone — proves the fan-out path stamped it"
    );
}

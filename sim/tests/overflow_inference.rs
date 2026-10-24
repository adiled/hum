//! A prompt goes to a worker with room. humd hosts no cells; each worker
//! claims its own headroom, and a full one is passed over for one with
//! room on a peer.

use std::time::Duration;

const MODEL: &str = "claude-haiku-4-5";

async fn answer(sim: &sim::Sim, humd: ensemble::Hid, sid: &str) -> Option<String> {
    sim.nestler_send(
        humd,
        serde_json::json!({
            "chi": "prompt",
            "rid": format!("{sid}-rid"),
            "sid": sid,
            "modelId": MODEL,
            "cwd": "/tmp",
            "content": "Say who you are.",
        }),
    )
    .expect("nestler accepts prompt");
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut said = None;
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Some(tone) = sim.nestler_recv(humd, sid, remaining).await else { break };
        let chi = tone.get("chi").and_then(|v| v.as_str()).unwrap_or("");
        if chi == "chunk" && tone.get("chunkType").and_then(|v| v.as_str()) == Some("text_delta") {
            said = tone.get("delta").and_then(|v| v.as_str()).map(str::to_string);
        }
        if chi == "finish" {
            break;
        }
    }
    said
}

async fn pair(sim: &sim::Sim) -> (std::sync::Arc<sim::SimHumd>, std::sync::Arc<sim::SimHumd>) {
    let a = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    let b = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    sim.wire_unauthenticated(a.id, b.id).expect("wire A ↔ B");
    sim.attach_mock_worker_claiming(b.id, vec![MODEL.into()], Some((3, 4)), "B")
        .await
        .expect("worker with room on B");
    (a, b)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_full_worker_is_passed_over_for_one_with_room_on_a_peer() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();
    let (a, _b) = pair(&sim).await;
    sim.attach_mock_worker_claiming(a.id, vec![MODEL.into()], Some((0, 4)), "A")
        .await
        .expect("full worker on A");
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert_eq!(answer(&sim, a.id, "full-1").await.as_deref(), Some("B"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_that_fills_up_is_passed_over_from_its_next_prompt() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();
    let (a, _b) = pair(&sim).await;
    let local = sim
        .attach_mock_worker_claiming(a.id, vec![MODEL.into()], Some((2, 4)), "A")
        .await
        .expect("worker with room on A");
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert_eq!(answer(&sim, a.id, "fill-1").await.as_deref(), Some("A"));

    a.thrum
        .inject_tone(
            &local,
            serde_json::json!({
                "chi": "pulse",
                "rid": "fill-pulse",
                "kind": "cell-spawned",
                "headroom": {"freeSlots": 0, "totalSlots": 4},
            }),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(answer(&sim, a.id, "fill-2").await.as_deref(), Some("B"));
}

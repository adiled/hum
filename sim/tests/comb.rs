//! The comb records what crossed the kernel — tone counts per chi, and
//! nectar per bee. Only the kernel writes it, so the measure is observed
//! rather than self-reported.

use ensemble::Hid;
use sim::Sim;

fn comb_path(id: &Hid) -> std::path::PathBuf {
    std::env::temp_dir()
        .join(format!("sim-humd-{}", id.short()))
        .join(hum_paths::COMB_BASENAME)
}

fn read_comb(id: &Hid) -> std::collections::HashMap<String, u64> {
    let raw = std::fs::read_to_string(comb_path(id)).expect("comb.json written on shutdown");
    serde_json::from_str(&raw).expect("comb.json is a flat map of counters")
}

#[tokio::test(flavor = "multi_thread")]
async fn comb_counts_chi_and_measures_nectar() {
    let sim = Sim::new();
    let a = Hid::random_humd();
    let h = sim.spawn_humd(a).await;

    let client_id = sim
        .attach_mock_worker(a, vec!["claude-haiku-4-5".into()])
        .await
        .expect("mock worker attaches");

    h.thrum
        .inject_tone(
            &client_id,
            serde_json::json!({
                "chi": "finish",
                "rid": "f1",
                "sid": "test-hum-1",
                "finishReason": "end_turn",
                "usage": { "input_tokens": 1200, "output_tokens": 34 },
            }),
        )
        .await;
    h.thrum
        .inject_tone(
            &client_id,
            serde_json::json!({
                "chi": "tool-call",
                "rid": "t1",
                "sid": "test-hum-1",
                "callId": "c1",
                "name": "read",
                "args": {},
            }),
        )
        .await;

    sim.shutdown().await;

    let comb = read_comb(&a);
    assert_eq!(comb.get("chi:finish"), Some(&1));
    assert_eq!(comb.get("chi:tool-call"), Some(&1));
    assert_eq!(comb.get("nectar:in"), Some(&1200));
    assert_eq!(comb.get("nectar:out"), Some(&34));
}

#[tokio::test(flavor = "multi_thread")]
async fn comb_attributes_work_to_the_bee_that_did_it() {
    let sim = Sim::new();
    let a = Hid::random_humd();
    let h = sim.spawn_humd(a).await;

    let client_id = sim
        .attach_mock_worker(a, vec!["claude-haiku-4-5".into()])
        .await
        .expect("mock worker attaches");

    h.thrum
        .inject_tone(
            &client_id,
            serde_json::json!({
                "chi": "finish",
                "rid": "f1",
                "sid": "test-hum-1",
                "usage": { "input_tokens": 900 },
            }),
        )
        .await;

    sim.shutdown().await;

    let comb = read_comb(&a);
    assert_eq!(comb.get("bee:claude-repl:chi:finish"), Some(&1));
    assert_eq!(comb.get("bee:claude-repl:nectar:in"), Some(&900));
}

#[tokio::test(flavor = "multi_thread")]
async fn comb_does_not_invent_nectar_it_was_not_told() {
    let sim = Sim::new();
    let a = Hid::random_humd();
    let h = sim.spawn_humd(a).await;

    let client_id = sim
        .attach_mock_worker(a, vec!["claude-haiku-4-5".into()])
        .await
        .expect("mock worker attaches");

    h.thrum
        .inject_tone(
            &client_id,
            serde_json::json!({
                "chi": "finish",
                "rid": "f1",
                "sid": "test-hum-1",
                "usage": {},
            }),
        )
        .await;

    sim.shutdown().await;

    let comb = read_comb(&a);
    assert_eq!(comb.get("chi:finish"), Some(&1));
    assert_eq!(comb.get("nectar:in"), None);
}

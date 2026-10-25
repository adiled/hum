//! peer_attendants — an attendant on one humd serves the blooms of a peer
//! that trusts remote attendants.
//!
//! The laptop's bloom is handed the tools a peer's attendants serve, after
//! its own attendants' and its forager's, and a reach for one goes to that
//! peer and comes back with what it grasped. Without that trust, or for a
//! hive the laptop was never taught, the peer's tools are neither offered
//! nor reached: the reach is answered unserved here.

use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::mpsc::Receiver;

async fn nestle(humd: &sim::SimHumd, hello: Value) -> (String, Receiver<Value>) {
    for _ in 0..200 {
        if humd.thrum.has_sink() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let cid = hum_identity::HumId::mint().to_string();
    let rx = humd.thrum.register_synthetic(cid.clone());
    humd.thrum.inject_tone(&cid, humd.signed_hello(hello)).await;
    (cid, rx)
}

async fn nurse(humd: &sim::SimHumd) -> (String, Receiver<Value>) {
    nestle(humd, json!({
        "chi": "hello", "role": "nurse", "hive": "claude-cli", "version": "0.0.0",
        "protoVersion": thrum_core::THRUM_VERSION,
        "models": ["claude-opus-4-7"],
        "chis": ["hello", "prompt", "unfurl", "tendril-reach", "wilt"],
    }))
    .await
}

async fn forager(humd: &sim::SimHumd) -> (String, Receiver<Value>) {
    nestle(humd, json!({
        "chi": "hello", "role": "forager", "hive": "openai-server", "version": "0.0.0",
        "protoVersion": thrum_core::THRUM_VERSION,
    }))
    .await
}

async fn heard(rx: &mut Receiver<Value>, chi: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let t = rx.recv().await.expect("still connected");
            if t.get("chi").and_then(Value::as_str) == Some(chi) {
                return t;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("never heard chi:{chi}"))
}

fn names(prompt: &Value) -> Vec<String> {
    prompt
        .get("tools")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|t| t.get("name").and_then(Value::as_str).map(str::to_string)).collect())
        .unwrap_or_default()
}

struct Pair {
    _sim: sim::Sim,
    laptop: std::sync::Arc<sim::SimHumd>,
    nurse: String,
    nurse_rx: Receiver<Value>,
    forager: String,
}

async fn pair(trust: sim::Trust, laptop_knows_the_hive: bool) -> Pair {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();
    let laptop = sim.spawn_humd_trusting(ensemble::Hid::random_humd(), trust).await;
    let server = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    sim.wire_unauthenticated(laptop.id, server.id).expect("L↔S");
    if laptop_knows_the_hive {
        laptop.hives.install("paid-oracle");
    }
    sim.attach_mock_tools(server.id, "paid-oracle", vec!["quote".into()], |_, args| format!("42, for {args}"))
        .await
        .expect("an attendant serves quote on the server");
    tokio::time::sleep(Duration::from_millis(400)).await;
    let (nurse, nurse_rx) = nurse(&laptop).await;
    let (forager, _forager_rx) = forager(&laptop).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    Pair { _sim: sim, laptop, nurse, nurse_rx, forager }
}

impl Pair {
    async fn bloom(&mut self, sid: &str) -> Value {
        let tone = json!({ "chi": "prompt", "rid": format!("r-{sid}"), "sid": sid, "modelId": "claude-opus-4-7", "text": "hi" });
        self.laptop.thrum.inject_tone(&self.forager, tone).await;
        heard(&mut self.nurse_rx, "prompt").await
    }

    async fn reach(&mut self, sid: &str, name: &str) -> Value {
        let tone = json!({ "chi": "tendril-reach", "sid": sid, "callId": format!("call-{sid}"), "name": name, "args": { "symbol": "HUM" } });
        self.laptop.thrum.inject_tone(&self.nurse, tone).await;
        heard(&mut self.nurse_rx, "tendril-result").await
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_peers_attendant_serves_a_bloom_here_when_this_humd_trusts_it() {
    let mut p = pair(sim::Trust::ALL, true).await;
    let handed = p.bloom("peer-tools-1").await;
    assert_eq!(names(&handed), ["quote"], "the peer's attendant's tool is handed to the bloom");
    assert_eq!(handed["tools"][0]["description"], "mock quote");

    let grasped = p.reach("peer-tools-1", "quote").await;
    assert_eq!(grasped["callId"], "call-peer-tools-1");
    assert_ne!(grasped["isError"], true, "{grasped}");
    let output = grasped["output"].as_str().or_else(|| grasped["result"].as_str()).unwrap_or("");
    assert!(output.starts_with("42") && output.contains("HUM"), "the reach went to the peer with its input: {grasped}");
}

#[tokio::test(flavor = "multi_thread")]
async fn without_that_trust_a_peers_attendant_is_neither_offered_nor_reached() {
    let mut p = pair(sim::Trust { attendants: false, ..sim::Trust::ALL }, true).await;
    let handed = p.bloom("peer-tools-2").await;
    assert!(names(&handed).is_empty(), "nothing of the peer's is offered: {:?}", names(&handed));
    let grasped = p.reach("peer-tools-2", "quote").await;
    assert_eq!(grasped["isError"], true);
    assert!(grasped["output"].as_str().unwrap_or("").contains("no bee serves quote"), "{grasped}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_peers_attendant_of_a_hive_this_humd_was_never_taught_is_not_offered() {
    let mut p = pair(sim::Trust::ALL, false).await;
    let handed = p.bloom("peer-tools-3").await;
    assert!(names(&handed).is_empty(), "{:?}", names(&handed));
    let grasped = p.reach("peer-tools-3", "quote").await;
    assert_eq!(grasped["isError"], true);
}

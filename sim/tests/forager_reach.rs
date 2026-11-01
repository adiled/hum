//! forager_reach — what a forager can reach, and the answers that come
//! back to whoever reached.
//!
//! A forager's breath says what it can reach: the `models` nurses grow,
//! the `scents` attendants distil, and the `tools` attendants serve its
//! blooms, and humd breathes again when that changes. A nurse's reach for a
//! tool the forager offered comes back to that nurse when the forager
//! answers. A `scent` reach goes to the attendant that distils its model,
//! from a forager with no bloom between, and is never handed to a bloom. A
//! prompt that names no model goes to any nurse with room. A cancel
//! follows its bloom to the peer whose nurse grows it.

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

async fn nurse(humd: &sim::SimHumd, hive: &str, model: &str) -> (String, Receiver<Value>) {
    nestle(humd, json!({
        "chi": "hello", "role": "nurse", "hive": hive, "version": "0.0.0",
        "protoVersion": thrum_core::THRUM_VERSION,
        "models": [model],
    }))
    .await
}

async fn attendant(humd: &sim::SimHumd, models: Value, tools: Value) -> (String, Receiver<Value>) {
    nestle(humd, json!({
        "chi": "hello", "role": "attendant", "hive": "openai-server", "version": "0.0.0",
        "protoVersion": thrum_core::THRUM_VERSION,
        "models": models,
        "tools": tools,
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
    heard_where(rx, chi, |_| true).await
}

async fn heard_where(rx: &mut Receiver<Value>, chi: &str, wanted: impl Fn(&Value) -> bool) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let t = rx.recv().await.expect("still connected");
            if t.get("chi").and_then(Value::as_str) == Some(chi) && wanted(&t) {
                return t;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("never heard chi:{chi}"))
}

fn ids(list: &Value) -> Vec<String> {
    list.as_array()
        .map(|a| a.iter().filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_string)).collect())
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_forager_tool_comes_back_to_the_nurse_that_reached_it() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();
    let humd = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    let (n, mut nurse_rx) = nurse(&humd, "claude-cli", "claude-opus-4-7").await;
    let (f, mut forager_rx) = forager(&humd).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    humd.thrum
        .inject_tone(&f, json!({
            "chi": "prompt", "rid": "r-1", "sid": "hum-back-1", "modelId": "claude-opus-4-7",
            "content": [{ "kind": "text", "text": "find it" }],
            "tools": [{ "name": "search", "inputSchema": { "type": "object" } }],
        }))
        .await;
    heard(&mut nurse_rx, "prompt").await;
    humd.thrum
        .inject_tone(&n, json!({
            "chi": "tendril-reach", "sid": "hum-back-1", "callId": "call-search",
            "name": "search", "args": { "q": "bees" },
        }))
        .await;
    let reach = heard(&mut forager_rx, "tendril-reach").await;
    assert_eq!(reach["callId"], "call-search");

    humd.thrum
        .inject_tone(&f, json!({
            "chi": "tendril-result", "sid": "hum-back-1", "callId": "call-search", "output": "three hives",
        }))
        .await;
    let grasped = heard(&mut nurse_rx, "tendril-result").await;
    assert_eq!(grasped["callId"], "call-search");
    assert_eq!(grasped["output"], "three hives", "the forager's answer reaches the nurse that asked");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_scent_reaches_the_attendant_that_distils_its_model() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();
    let humd = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    let (a, mut attendant_rx) = attendant(&humd, json!(["embed-small"]), json!([])).await;
    let (f, mut forager_rx) = forager(&humd).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    humd.thrum
        .inject_tone(&f, json!({
            "chi": "tendril-reach", "callId": "call-scent",
            "name": "scent", "args": { "model": "embed-small", "input": ["a bee", "a wasp"] },
        }))
        .await;
    let reach = heard(&mut attendant_rx, "tendril-reach").await;
    assert_eq!(reach["args"]["input"][1], "a wasp");
    humd.thrum
        .inject_tone(&a, json!({
            "chi": "tendril-result", "callId": "call-scent",
            "result": { "scents": [[1.0, 0.0], [0.9, 0.1]], "nectar": { "input_tokens": 4 } },
        }))
        .await;
    let grasped = heard(&mut forager_rx, "tendril-result").await;
    assert_eq!(grasped["result"]["scents"][1][0], 0.9);

    humd.thrum
        .inject_tone(&f, json!({
            "chi": "tendril-reach", "callId": "call-nowhere",
            "name": "scent", "args": { "model": "no-such-embedder", "input": ["a"] },
        }))
        .await;
    let unserved = heard_where(&mut forager_rx, "tendril-result", |t| t["callId"] == "call-nowhere").await;
    assert_eq!(unserved["isError"], true);
    assert!(unserved["output"].as_str().unwrap().contains("no-such-embedder"));
    assert!(unserved.get("sid").is_none(), "a reach with no bloom is answered with none");
}

#[tokio::test(flavor = "multi_thread")]
async fn scent_and_bash_are_never_an_attendants_to_hand_a_bloom() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();
    let humd = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    let (_a, _attendant_rx) = attendant(
        &humd,
        json!(["embed-small"]),
        json!([{ "name": "scent" }, { "name": "bash" }, { "name": "wrepl" }]),
    )
    .await;
    let (_n, mut nurse_rx) = nurse(&humd, "claude-cli", "claude-opus-4-7").await;
    let (f, _forager_rx) = forager(&humd).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    humd.thrum
        .inject_tone(&f, json!({
            "chi": "prompt", "rid": "r-2", "sid": "hum-handed-1", "modelId": "claude-opus-4-7", "text": "hi",
        }))
        .await;
    let handed = heard(&mut nurse_rx, "prompt").await;
    let names: Vec<&str> = handed["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
    assert_eq!(names, ["wrepl"], "the thrum's own tendrils are never an attendant's to offer");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_forager_hears_what_it_can_reach_and_hears_again_when_it_changes() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();
    let humd = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    let (_n, _nurse_rx) = nurse(&humd, "claude-cli", "claude-opus-4-7").await;
    let (_a, _attendant_rx) = attendant(
        &humd,
        json!(["embed-small"]),
        json!([{ "name": "wrepl", "description": "the web, as a command line", "inputSchema": { "type": "object" } }]),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (_f, mut forager_rx) = forager(&humd).await;

    let first = heard(&mut forager_rx, "breath").await;
    assert_eq!(ids(&first["models"]), ["claude-opus-4-7"]);
    assert_eq!(first["models"][0]["hive"], "claude-cli");
    assert_eq!(ids(&first["scents"]), ["embed-small"]);
    assert_eq!(first["tools"][0]["name"], "wrepl");

    let (_m, _more_rx) = nurse(&humd, "openai-server", "gpt-lab").await;
    let changed = heard_where(&mut forager_rx, "breath", |b| ids(&b["models"]).len() == 2).await;
    assert_eq!(ids(&changed["models"]), ["claude-opus-4-7", "gpt-lab"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_that_names_no_model_goes_to_any_nurse() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();
    let humd = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    let (_n, mut nurse_rx) = nurse(&humd, "claude-cli", "claude-opus-4-7").await;
    let (f, mut forager_rx) = forager(&humd).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    humd.thrum
        .inject_tone(&f, json!({ "chi": "prompt", "rid": "r-3", "sid": "hum-any-1", "text": "hi" }))
        .await;
    let handed = heard(&mut nurse_rx, "prompt").await;
    assert_eq!(handed["sid"], "hum-any-1");

    humd.thrum
        .inject_tone(&f, json!({ "chi": "prompt", "rid": "r-4", "sid": "hum-none-1", "modelId": "nobody-grows-this", "text": "hi" }))
        .await;
    let refused = heard(&mut forager_rx, "error").await;
    assert_eq!(refused["code"], "no-nurse");
    assert!(refused["message"].as_str().unwrap().contains("nobody-grows-this"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_follows_its_bloom_to_the_peer_that_grows_it() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();
    let laptop = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    let server = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    sim.wire_unauthenticated(laptop.id, server.id).expect("L↔S");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (_n, mut nurse_rx) = nurse(&server, "claude-cli", "claude-opus-4-7").await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    sim.nestler_send(laptop.id, json!({
        "chi": "prompt", "rid": "r-far", "sid": "hum-far-1", "modelId": "claude-opus-4-7", "text": "think long",
    }))
    .expect("the laptop's forager prompts");
    heard(&mut nurse_rx, "prompt").await;

    sim.nestler_send(laptop.id, json!({ "chi": "cancel", "rid": "r-stop", "sid": "hum-far-1" }))
        .expect("the laptop's forager cancels");
    let cancel = heard(&mut nurse_rx, "cancel").await;
    assert_eq!(cancel["sid"], "hum-far-1", "the nurse on the peer hears the cancel of the bloom it grows");
}

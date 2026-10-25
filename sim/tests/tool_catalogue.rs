//! tool_catalogue — what humd hands a nurse with a prompt, and where a
//! bloom's `bash` lands.
//!
//! The prompt a nurse receives carries in `tools` everything its bloom may
//! reach: `bash` when the bloom builds somewhere, every attendant's tools,
//! then the forager's own, each name once. A bloom builds where its `cwd`
//! says, `hum://<humdid>/<path>` or `hum:///<path>` for the forager's own
//! humd, or at the forager's humd's home when its hello says `builds`.
//! Anywhere else, `bash` is answered unserved, unless the forager offered
//! a `bash` of its own; a `cwd` that is not `hum://` is dropped with an
//! advisory.

use std::sync::Arc;
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

async fn forager(humd: &sim::SimHumd, builds: bool) -> (String, Receiver<Value>) {
    nestle(humd, json!({
        "chi": "hello", "role": "forager", "hive": "openai-server", "version": "0.0.0",
        "protoVersion": thrum_core::THRUM_VERSION,
        "builds": builds,
    }))
    .await
}

async fn heard(rx: &mut Receiver<Value>, chi: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(2), async {
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

async fn prompt(humd: &sim::SimHumd, forager: &str, sid: &str, extra: Value) {
    let mut tone = json!({
        "chi": "prompt", "rid": format!("r-{sid}"), "sid": sid,
        "modelId": "claude-opus-4-7", "text": "hi",
    });
    if let (Some(obj), Value::Object(extra)) = (tone.as_object_mut(), extra) {
        obj.extend(extra);
    }
    humd.thrum.inject_tone(forager, tone).await;
}

async fn reach_bash(humd: &sim::SimHumd, nurse: &str, sid: &str, command: &str) {
    humd.thrum
        .inject_tone(nurse, json!({
            "chi": "tendril-reach", "sid": sid, "callId": format!("call-{sid}"),
            "name": "bash", "args": { "command": command },
        }))
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bloom_is_handed_every_tool_it_may_reach() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();
    let humd = sim.spawn_humd(ensemble::Hid::random_humd()).await;

    sim.attach_mock_tools(humd.id, "paid-oracle", vec!["quote".into()], |_, _| "42".into())
        .await
        .expect("an attendant serves quote");
    let (_n, mut nurse_rx) = nurse(&humd).await;
    let (f, _forager_rx) = forager(&humd, true).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    prompt(&humd, &f, "hum-tools-1", json!({
        "tools": [
            { "name": "read", "description": "file read",
              "inputSchema": { "type": "object", "properties": { "path": { "type": "string" } } } },
            { "name": "quote", "description": "the forager's own quote" },
        ],
    }))
    .await;

    let handed = heard(&mut nurse_rx, "prompt").await;
    assert_eq!(names(&handed), ["bash", "quote", "read"], "bash, the attendant's, then the forager's, each once");
    for t in handed["tools"].as_array().unwrap() {
        assert!(t["inputSchema"].is_object(), "every tool has an object inputSchema: {t}");
    }
    assert_eq!(handed["tools"][1]["description"], "mock quote", "the attendant's quote wins the name");
    assert_eq!(handed["cwd"], format!("hum://{}/", humd.id.to_hex()), "a forager that builds builds at home");
    for gone in ["foragerTools", "provided", "disallowedTools"] {
        assert!(handed.get(gone).is_none(), "{gone} is not on the wire");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn bash_reaches_the_builder_where_the_bloom_builds() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();
    let humd = sim.spawn_humd(ensemble::Hid::random_humd()).await;

    let ran: Arc<parking_lot::Mutex<Vec<String>>> = Arc::default();
    let ran_here = ran.clone();
    sim.attach_mock_builder(humd.id, move |cwd, command| {
        ran_here.lock().push(format!("{cwd}$ {command}"));
        ("nope".into(), 2)
    })
    .await
    .expect("a builder lends this host");
    let (n, mut nurse_rx) = nurse(&humd).await;
    let (f, _forager_rx) = forager(&humd, false).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    prompt(&humd, &f, "hum-builds-1", json!({ "cwd": "hum:///repo" })).await;
    let handed = heard(&mut nurse_rx, "prompt").await;
    assert_eq!(names(&handed), ["bash"]);
    assert_eq!(handed["cwd"], format!("hum://{}/repo", humd.id.to_hex()), "here resolves to this humd");

    reach_bash(&humd, &n, "hum-builds-1", "ls missing").await;
    let grasped = heard(&mut nurse_rx, "tendril-result").await;
    assert_eq!(grasped["output"], "nope");
    assert_eq!(grasped["isError"], true, "a non-zero exit is an error");
    assert_eq!(grasped["metadata"]["exitCode"], 2);
    assert_eq!(ran.lock().as_slice(), ["/repo$ ls missing"]);

    prompt(&humd, &f, "hum-builds-1", json!({})).await;
    let again = heard(&mut nurse_rx, "prompt").await;
    assert_eq!(again["cwd"], handed["cwd"], "a sigil keeps where it builds until told otherwise");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bloom_that_builds_nowhere_hears_bash_answered_unserved() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();
    let humd = sim.spawn_humd(ensemble::Hid::random_humd()).await;

    let (n, mut nurse_rx) = nurse(&humd).await;
    let (f, _forager_rx) = forager(&humd, false).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    prompt(&humd, &f, "hum-nowhere-1", json!({})).await;
    let handed = heard(&mut nurse_rx, "prompt").await;
    assert!(names(&handed).is_empty(), "no bash for a bloom that builds nowhere");
    assert!(handed.get("cwd").is_none());

    reach_bash(&humd, &n, "hum-nowhere-1", "ls").await;
    let grasped = heard(&mut nurse_rx, "tendril-result").await;
    assert_eq!(grasped["isError"], true);
    assert!(grasped["output"].as_str().unwrap().contains("nowhere to run"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bloom_built_where_no_builder_is_hears_bash_answered_unserved() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();
    let humd = sim.spawn_humd(ensemble::Hid::random_humd()).await;

    let (n, mut nurse_rx) = nurse(&humd).await;
    let (f, _forager_rx) = forager(&humd, true).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    prompt(&humd, &f, "hum-unbuilt-1", json!({})).await;
    heard(&mut nurse_rx, "prompt").await;
    reach_bash(&humd, &n, "hum-unbuilt-1", "ls").await;
    let grasped = heard(&mut nurse_rx, "tendril-result").await;
    assert_eq!(grasped["isError"], true);
    assert!(grasped["output"].as_str().unwrap().contains("no builder"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_forager_that_offers_its_own_bash_is_reached_when_the_bloom_builds_nowhere() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();
    let humd = sim.spawn_humd(ensemble::Hid::random_humd()).await;

    let (n, mut nurse_rx) = nurse(&humd).await;
    let (f, mut forager_rx) = forager(&humd, false).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    prompt(&humd, &f, "hum-own-bash-1", json!({
        "tools": [{ "name": "bash", "inputSchema": { "type": "object" } }],
    }))
    .await;
    let handed = heard(&mut nurse_rx, "prompt").await;
    assert_eq!(names(&handed), ["bash"]);
    assert_eq!(handed["tools"][0]["inputSchema"], json!({ "type": "object" }), "the forager's own bash");

    reach_bash(&humd, &n, "hum-own-bash-1", "ls").await;
    let reach = heard(&mut forager_rx, "tendril-reach").await;
    assert_eq!(reach["args"]["command"], "ls");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cwd_that_is_not_hum_is_dropped_with_an_advisory() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();
    let humd = sim.spawn_humd(ensemble::Hid::random_humd()).await;

    let (_n, mut nurse_rx) = nurse(&humd).await;
    let (f, mut forager_rx) = forager(&humd, false).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    prompt(&humd, &f, "hum-plain-1", json!({ "cwd": "/Users/op/repo" })).await;
    let handed = heard(&mut nurse_rx, "prompt").await;
    assert!(handed.get("cwd").is_none(), "a plain path is dropped");
    let advised = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let t = forager_rx.recv().await.expect("still connected");
            if t.pointer("/fields/advisory").and_then(Value::as_str) == Some("cwd-not-hum") {
                return t;
            }
        }
    })
    .await
    .expect("the forager is advised");
    assert_eq!(advised["fields"]["cwd"], "/Users/op/repo");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reach_no_bee_serves_is_answered_unserved() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();
    let humd = sim.spawn_humd(ensemble::Hid::random_humd()).await;

    let (n, mut nurse_rx) = nurse(&humd).await;
    let (f, mut forager_rx) = forager(&humd, false).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    prompt(&humd, &f, "hum-unknown-1", json!({ "tools": [{ "name": "search" }] })).await;
    heard(&mut nurse_rx, "prompt").await;
    humd.thrum
        .inject_tone(&n, json!({
            "chi": "tendril-reach", "sid": "hum-unknown-1", "callId": "call-ghost",
            "name": "ghost", "args": {},
        }))
        .await;
    let grasped = heard(&mut nurse_rx, "tendril-result").await;
    assert_eq!(grasped["isError"], true);
    assert!(grasped["output"].as_str().unwrap().contains("no bee serves ghost"));

    humd.thrum
        .inject_tone(&n, json!({
            "chi": "tendril-reach", "sid": "hum-unknown-1", "callId": "call-search",
            "name": "search", "args": {},
        }))
        .await;
    let reach = heard(&mut forager_rx, "tendril-reach").await;
    assert_eq!(reach["name"], "search", "the forager's own tool reaches back to it");
}

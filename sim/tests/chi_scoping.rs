//! Chi-scoped delivery.
//!
//! A bee that declares `chis` in its hello gets only those from a
//! broadcast. One that declares nothing still gets everything, because
//! every bee written before this field existed sends an empty set.

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use sim::{Sim, SimHumd};

fn base_hello(chis: Option<&[&str]>) -> Value {
    let mut hello = serde_json::json!({
        "chi": "hello",
        "bee": ["worker"],
        "hive": "claude-cli",
        "version": "0.1.0",
        "protoVersion": thrum_core::THRUM_VERSION,
    });
    if let Some(list) = chis {
        hello["chis"] = serde_json::json!(list);
    }
    hello
}

fn peer_reply(chi: &str, sid: &str) -> Value {
    serde_json::json!({
        "chi": chi,
        "from": "ensemble",
        "sid": sid,
        "text": "hello from the other side",
    })
}

fn drain(rx: &mut tokio::sync::mpsc::Receiver<Value>) -> Vec<Value> {
    let mut out = Vec::new();
    while let Ok(tone) = rx.try_recv() {
        out.push(tone);
    }
    out
}

fn chis_of(tones: &[Value]) -> Vec<&str> {
    tones
        .iter()
        .filter_map(|t| t.get("chi").and_then(Value::as_str))
        .collect()
}

async fn spawn() -> (Sim, Arc<SimHumd>) {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = Sim::new();
    let humd = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    sim.await_ready(humd.id).await.expect("humd ready");
    (sim, humd)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_scoped_client_does_not_receive_an_undeclared_chi() {
    let (_sim, humd) = spawn().await;

    let cid = hum_identity::HumId::mint().to_string();
    let mut rx = humd.thrum.register_synthetic(cid.clone());
    humd.thrum
        .inject_tone(&cid, base_hello(Some(&["prompt"])))
        .await;
    humd.thrum
        .inject_tone("ensemble", peer_reply("chunk", "sess-1"))
        .await;
    tokio::time::sleep(Duration::from_millis(150)).await;

    let got = drain(&mut rx);
    assert!(
        !chis_of(&got).contains(&"chunk"),
        "a client that declared only 'prompt' must not receive a chunk, got {:?}",
        chis_of(&got)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_scoped_client_receives_a_declared_chi() {
    let (_sim, humd) = spawn().await;

    let cid = hum_identity::HumId::mint().to_string();
    let mut rx = humd.thrum.register_synthetic(cid.clone());
    humd.thrum
        .inject_tone(&cid, base_hello(Some(&["prompt", "chunk"])))
        .await;

    humd.thrum
        .inject_tone("ensemble", peer_reply("chunk", "sess-1"))
        .await;
    tokio::time::sleep(Duration::from_millis(150)).await;

    let got = drain(&mut rx);
    assert!(
        chis_of(&got).contains(&"chunk"),
        "a client that declared 'chunk' must receive it, got {:?}",
        chis_of(&got)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_declared_nothing_still_receives_everything() {
    let (_sim, humd) = spawn().await;

    let cid = hum_identity::HumId::mint().to_string();
    let mut rx = humd.thrum.register_synthetic(cid.clone());
    humd.thrum.inject_tone(&cid, base_hello(None)).await;

    humd.thrum
        .inject_tone("ensemble", peer_reply("chunk", "sess-1"))
        .await;
    tokio::time::sleep(Duration::from_millis(150)).await;

    let got = drain(&mut rx);
    assert!(
        chis_of(&got).contains(&"chunk"),
        "a client that declared no chis must receive a chunk, got {:?}",
        chis_of(&got)
    );
}

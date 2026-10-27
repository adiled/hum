//! A connection on humd's socket is heard only once a breath admits its
//! hello, and a bee's newer connection replaces its older one.

mod common;

use common::*;
use serde_json::json;
use thrum_core::Role;

#[tokio::test(flavor = "multi_thread")]
async fn a_connection_is_heard_only_once_its_hello_is_admitted() {
    let humd = Humd::start().await;
    let (_, mut n_rx) = humd.nurse().await;
    let mut w = Wire::connect(&humd.socket().await).await;
    let first = w.next(WAIT).await.unwrap();
    assert_eq!(first["sigils"], json!([]), "the breath before a hello admits nothing");

    for tone in [
        json!({"chi": "prompt", "rid": "r1", "sid": "x", "modelId": MODEL, "text": "hi"}),
        json!({"chi": "attach", "rid": "r2", "sid": "y"}),
        json!({"chi": "cancel", "rid": "r3", "sid": "x"}),
        json!({"chi": "cleanup", "rid": "r4", "sid": "x"}),
        json!({"chi": "curate", "rid": "r5", "sid": "x"}),
        json!({"chi": "tendril-reach", "rid": "r6", "sid": "x", "callId": "c1", "name": TOOL, "args": {}}),
        json!({"chi": "tendril-result", "rid": "r7", "sid": "x", "callId": "c1", "output": "o"}),
        json!({"chi": "error", "rid": "r8", "sid": "x", "message": "m"}),
        json!({"chi": "unfurl", "rid": "r9", "sid": "x", "petal": 0, "kind": "text", "text": "t"}),
        json!({"chi": "backfill", "rid": "r10", "author": "a", "from": 0}),
    ] {
        let rid = tone["rid"].clone();
        w.send(tone).await;
        let answer = w.next(WAIT).await.expect("humd answers a tone before a hello");
        assert_eq!(answer["chi"], "echo");
        assert_eq!(answer["rid"], rid);
        assert_eq!(answer["ok"], false, "humd took a tone before a hello: {answer}");
    }
    for chi in ["prompt", "cancel", "cleanup"] {
        let heard = heard_within(&mut n_rx, chi, "x", QUIET).await;
        assert!(heard.is_none(), "a nurse heard a {chi} from a connection that never said hello");
    }
    assert!(!humd.thrum.holds("x") && !humd.thrum.holds("y"));
    assert_eq!(humd.wane_of("x"), None);

    w.send(hello(&humd.enter(Role::Forager), Role::Forager, json!({}))).await;
    w.nestled().await;
    w.send(json!({"chi": "prompt", "rid": "r11", "sid": "z", "modelId": MODEL, "text": "hi"})).await;
    hear(&mut n_rx, "prompt", "z").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bees_newer_connection_replaces_its_older_one() {
    let humd = Humd::start().await;
    let (_, mut n_rx) = humd.nurse().await;
    let sock = humd.socket().await;
    let key = humd.enter(Role::Forager);
    let mut older = Wire::connect(&sock).await;
    older.send(hello(&key, Role::Forager, json!({}))).await;
    older.nestled().await;
    older.send(prompt("old")).await;
    hear(&mut n_rx, "prompt", "old").await;
    assert!(humd.thrum.holds("old"));

    let mut newer = Wire::connect(&sock).await;
    newer.send(hello(&key, Role::Forager, json!({}))).await;
    newer.nestled().await;
    assert!(older.closed().await, "the older connection is still open");
    let since = tokio::time::Instant::now();
    while humd.thrum.holds("old") || humd.wane_of("old").is_some() {
        assert!(since.elapsed() < WAIT, "the older connection's sid outlived it");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    older.send(prompt("after")).await;
    assert!(heard_within(&mut n_rx, "prompt", "after", QUIET).await.is_none(), "the replaced connection still prompts");
    newer.send(prompt("new")).await;
    hear(&mut n_rx, "prompt", "new").await;
}

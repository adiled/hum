//! The nurse as humd reaches it: a forager's prompt goes through humd to
//! the nurse and its bloom comes back, a sigil keeps its conversation, a
//! cancel kills the bridge, and a nurse whose Mac cannot grow yet waits
//! before it nestles.

mod common;

use std::time::Duration;

use common::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_through_humd_unfurls_and_wilts_back() {
    let stub = Stub::new();
    let nurse = Nurse::spawn("grow", &stub.bridge(), &[]).nestled().await;
    let mut f = Forager::link("grow").await;
    f.breathed(&nurse.model).await;

    f.send(json!({"chi": "prompt", "sid": "a", "modelId": nurse.model, "systemPrompt": "be brief",
        "content": [{"kind": "text", "text": "hello"}]}));
    let bloom = f.bloom("a").await;
    assert_eq!((bloom.said.as_str(), bloom.unfurls), ("heard hello. ", 2), "each delta unfurls as it comes");
    assert_eq!(bloom.end["reason"], "stop");
    assert_eq!(bloom.end["nectar"], json!({"input_tokens": 7, "output_tokens": 3}));

    f.prompt("a", &nurse.model, "again");
    assert_eq!(f.bloom("a").await.said, "heard again. ");
    let asked = stub.asked();
    assert_eq!(asked[0]["instructions"], "be brief");
    assert_eq!(
        asked[1]["turns"],
        json!([{"prompt": "hello", "response": "heard hello. "}]),
        "the sigil's next prompt, sent with no graft, grows on from what its cell grew"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_through_humd_kills_the_bridge() {
    let stub = Stub::new();
    let nurse = Nurse::spawn("cancel", &stub.bridge(), &[]).nestled().await;
    let mut f = Forager::link("cancel").await;
    f.breathed(&nurse.model).await;

    f.prompt("c", &nurse.model, "hang on");
    let first = f.on("c").await;
    assert_eq!((first["chi"].clone(), first["text"].clone()), (json!("unfurl"), json!("hanging")));
    assert!(stub.alive());
    f.send(json!({"chi": "cancel", "sid": "c"}));
    assert_eq!(f.bloom("c").await.ended(), "wilt cancel");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!stub.alive(), "the bridge is killed with its bloom");

    f.prompt("c", &nurse.model, "after");
    assert_eq!(f.bloom("c").await.said, "heard after. ", "the sigil grows on in a fresh cell");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_nurse_waits_unnestled_until_apple_s_model_is_ready() {
    let stub = Stub::new();
    stub.probe(false, "model_not_ready");
    let nurse = Nurse::spawn("dormant", &stub.bridge(), &[("APPLE_FM_PROBE_MS", "100")]);
    nurse.wait_for("apple-fm.dormant").await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    let log = nurse.log_text();
    assert!(!log.contains("nurse.nestled"), "a dormant nurse does not nestle:\n{log}");
    assert_eq!(log.matches("apple-fm.dormant").count(), 1, "it says so once, not at every probe:\n{log}");
    assert!(log.contains("reason=model_not_ready"), "{log}");

    let mut f = Forager::link("dormant").await;
    stub.probe(true, "");
    let nurse = nurse.nestled().await;
    f.breathed(&nurse.model).await;
    f.prompt("d", &nurse.model, "awake");
    assert_eq!(f.bloom("d").await.said, "heard awake. ");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mac_that_cannot_run_the_model_keeps_its_unit_down() {
    let stub = Stub::new();
    stub.probe(false, "device_not_eligible");
    let mut nurse = Nurse::spawn("never", &stub.bridge(), &[("APPLE_FM_PROBE_MS", "100")]);
    assert!(nurse.exit().await.success(), "it exits 0, so orchd does not restart it:\n{}", nurse.log_text());
    let log = nurse.log_text();
    assert!(log.contains("apple-fm.never") && !log.contains("nurse.nestled"), "{log}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bridge_that_cannot_run_fails_the_unit() {
    world();
    let mut nurse = Nurse::spawn("missing", std::path::Path::new("/nonexistent/apple-fm-gen"), &[]);
    assert!(!nurse.exit().await.success(), "a missing bridge is an install gone wrong:\n{}", nurse.log_text());
    assert!(nurse.log_text().contains("/nonexistent/apple-fm-gen"), "{}", nurse.log_text());
}

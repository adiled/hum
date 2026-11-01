//! The forager as a gRPC client meets it: a stream that opens, prompts,
//! closes its side while blooms grow, goes away mid-bloom, shares the bee
//! with other streams, or is refused.

mod common;

use common::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn a_stream_opens_with_a_breath_and_grows_a_bloom() {
    let f = Forager::start("plain", &[]).await;
    let mut s = f.open().await;
    let first = s.next().await.unwrap().expect("a tone");
    assert_eq!(first["chi"], "breath", "{first}");
    s.send(prompt("plain-1", "hello")).await;
    let tones = s.until(is("wilt")).await;
    assert!(tones.iter().any(|t| t["chi"] == "echo" && t["rid"] == "plain-1-ask" && t["ok"] == true), "{tones:?}");
    assert!(tones.iter().any(|t| t["chi"] == "unfurl" && t["text"] == "echo: hello"), "{tones:?}");
    assert!(tones.iter().filter(|t| t["sid"].is_string()).all(|t| t["sid"] == "plain-1"), "{tones:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_empty_host_listens_on_loopback() {
    let f = Forager::start("loopback", &[("HUM_GRPC_HOST", "")]).await;
    f.wait_for(&format!("addr=127.0.0.1:{}", f.addr.port())).await;
    let mut s = f.open().await;
    assert_eq!(s.next().await.unwrap().expect("a tone")["chi"], "breath");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_stream_ends_with_why() {
    let elsewhere = world().root.join("no-hives");
    let f = Forager::start("refused", &[("XDG_CONFIG_HOME", elsewhere.to_str().unwrap())]).await;
    let mut s = f.open().await;
    let status = s.next().await.expect_err("a refusal is an error, not an empty stream");
    assert_eq!(status.code(), tonic::Code::FailedPrecondition, "{status:?}");
    assert!(status.message().contains("not installed"), "{}", status.message());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_half_closed_stream_ends_once_its_blooms_wilt() {
    let f = Forager::start("half", &[]).await;
    let mut s = f.open().await;
    s.until(is("breath")).await;
    s.send(prompt("half-1", "slow 300 one")).await;
    s.send(prompt("half-2", "slow 900 two")).await;
    s.close_send();
    let (tones, end) = s.rest().await;
    assert!(end.is_ok(), "{end:?}");
    let wilted: Vec<_> = tones.iter().filter(|t| t["chi"] == "wilt").map(|t| t["sid"].clone()).collect();
    assert_eq!(wilted, [json!("half-1"), json!("half-2")], "{tones:?}");
    assert!(tones.iter().any(|t| t["text"] == "echo: two"), "{tones:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stream_gone_mid_bloom_cancels_it() {
    let f = Forager::start("gone", &[]).await;
    let mut s = f.open().await;
    s.until(is("breath")).await;
    s.send(prompt("gone-1", "slow 5000 never heard")).await;
    s.until(|t| t["chi"] == "echo" && t["rid"] == "gone-1-ask").await;
    drop(s);
    let since = std::time::Instant::now();
    while !world().cancelled("gone-1") {
        assert!(since.elapsed() < WAIT, "the bloom was never cancelled\n{}", f.log_text());
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn every_stream_is_the_one_bee_and_hears_its_own() {
    let f = Forager::start("shared", &[]).await;
    let mut a = f.open().await;
    a.until(is("breath")).await;
    let mut b = f.open().await;
    b.until(is("breath")).await;

    world().nurse("nurse-later", "later");
    for s in [&mut a, &mut b] {
        s.until(|t| t["chi"] == "breath" && t["models"].to_string().contains("later")).await;
    }

    a.send(json!({"chi": "hello", "rid": "a-hello", "chis": ["breath"]})).await;
    let refused = a.until(|t| t["rid"] == "a-hello").await.pop().unwrap();
    assert_eq!(refused["ok"], false, "a stream does not change the bee's hello: {refused}");

    a.send(prompt("shared-a", "from a")).await;
    b.send(prompt("shared-b", "from b")).await;
    for (s, mine, theirs) in [(&mut a, "shared-a", "shared-b"), (&mut b, "shared-b", "shared-a")] {
        let tones = s.until(is("wilt")).await;
        assert_eq!(tones.last().unwrap()["sid"], mine, "{tones:?}");
        assert!(tones.iter().any(|t| t["chi"] == "echo" && t["rid"] == format!("{mine}-ask")), "{tones:?}");
        assert!(
            tones.iter().all(|t| t["sid"] != theirs && t["rid"] != format!("{theirs}-ask")),
            "a stream hears only its own: {tones:?}"
        );
    }
}

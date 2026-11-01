//! The forager across humd's absence. A stream stays open while humd is
//! away, and what it sends then waits for humd; a stream that has closed
//! its side and waits on blooms humd took with it ends, told why.

mod common;

use common::*;

#[tokio::test(flavor = "multi_thread")]
async fn streams_outlive_humd() {
    let f = Forager::start("outlive", &[]).await;
    let mut waiting = f.open().await;
    waiting.until(is("breath")).await;
    let mut open = f.open().await;
    open.until(is("breath")).await;

    waiting.send(prompt("outlive-lost", "slow 3000 lost with humd")).await;
    waiting.until(|t| t["chi"] == "echo" && t["rid"] == "outlive-lost-ask").await;
    waiting.close_send();
    world().stop_humd();
    let (_, end) = waiting.rest().await;
    let status = end.expect_err("a bloom humd took with it never wilts");
    assert_eq!(status.code(), tonic::Code::Unavailable, "{status:?}");

    open.send(prompt("outlive-later", "sent while humd was away")).await;
    world().start_humd();
    let tones = open.until(|t| t["chi"] == "echo" && t["rid"] == "outlive-later-ask").await;
    assert!(tones.iter().any(|t| t["chi"] == "breath"), "admitted again first: {tones:?}");
}

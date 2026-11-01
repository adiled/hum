//! The forager across humd's absence. A DTN node does not know or care
//! that the humd behind the forager restarted: an ask the forager took
//! is answered when humd is back, whether it arrived while humd was gone
//! or was blooming when humd went.

mod common;

use std::time::Duration;

use common::*;

#[tokio::test(flavor = "multi_thread")]
async fn asks_outlive_humd() {
    let f = Forager::start("outlive", &[]).await;
    let node = Node::new("outlive").await;

    node.send(&mut ask(&node.eid, &f.eid, b"slow 1500 mid-bloom-marker"), f.addr).await;
    let since = std::time::Instant::now();
    while world().heard("mid-bloom-marker").is_empty() {
        assert!(since.elapsed() < WAIT, "the nurse never heard it");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    world().stop_humd();
    node.send(&mut ask(&node.eid, &f.eid, b"while-away-marker"), f.addr).await;
    assert!(node.reply(Duration::from_secs(2)).await.is_none(), "nothing can bloom while humd is away");
    world().start_humd();

    let mut got: Vec<String> = node.replies(2, WAIT).await.into_iter().map(|r| r.text).collect();
    got.sort();
    assert_eq!(got, ["echo: mid-bloom-marker", "echo: while-away-marker"], "{}", f.log_text());
    assert!(node.reply(Duration::from_secs(3)).await.is_none(), "each answered once");
}

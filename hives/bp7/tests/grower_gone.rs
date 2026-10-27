//! The humd growing an ask goes away mid-bloom. The near humd breaks the
//! bloom off as `nurse-gone` when it loses that peer, and the ask is grown
//! again once that humd is back.

mod common;

use std::time::{Duration, Instant};

use common::near::{far_reach, forager_on, spawn_near};
use common::*;

#[tokio::test(flavor = "multi_thread")]
async fn an_ask_outlives_the_humd_growing_it() {
    let w = ensemble_world();
    let (far_hid, far_tcp) = far_reach();
    w.pin_tcp(&far_tcp);
    let near = spawn_near(&far_hid, &far_tcp).await;
    let f = forager_on(&near, "gone").await;
    let node = Node::new("gone").await;

    node.send(&mut ask(&node.eid, &f.eid, b"slow 3000 gone-marker"), f.addr).await;
    let since = Instant::now();
    while w.heard("gone-marker").is_empty() {
        assert!(since.elapsed() < WAIT, "the far nurse never heard it\n{}", f.log_text());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    w.stop_humd();
    tokio::time::sleep(Duration::from_millis(500)).await;
    w.start_humd();
    let reply = node.reply(Duration::from_secs(90)).await.unwrap_or_else(|| {
        panic!(
            "the ask hung when the humd growing it went away\n--- forager\n{}\n--- near humd\n{}",
            f.log_text(),
            near.log_text()
        )
    });
    assert_eq!(reply.text, "echo: gone-marker");
}

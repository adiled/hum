//! Space reaching into the ensemble. A DTN node's bundle lands on a
//! forager whose humd has no nurse at all; the only nurse that grows the
//! model is on another humd, reached over the ensemble, and the ask is
//! answered from there.

mod common;

use std::time::Duration;

use common::near::{far_reach, forager_on, spawn_near};
use common::*;

#[tokio::test(flavor = "multi_thread")]
async fn space_reaches_into_the_ensemble() {
    let w = ensemble_world();
    let (far_hid, far_tcp) = far_reach();
    let near = spawn_near(&far_hid, &far_tcp).await;
    let f = forager_on(&near, "reach").await;
    let node = Node::new("mars").await;

    for i in 0..3 {
        let words = format!("reach-marker-{i}: what is the speed of light in glass");
        node.send(&mut ask(&node.eid, &f.eid, words.as_bytes()), f.addr).await;
        let reply = node.reply(WAIT).await.unwrap_or_else(|| {
            panic!("no answer from the ensemble\n--- forager\n{}\n--- near humd\n{}", f.log_text(), near.log_text())
        });
        assert_eq!(reply.text, format!("echo: {words}"));
        let heard = w.heard(&format!("reach-marker-{i}:"));
        assert_eq!(heard.len(), 1, "grown once, by the far humd's nurse");
        assert!(heard[0].sid.starts_with("bp7-"), "{:?}", heard[0]);
    }
    assert!(node.reply(Duration::from_secs(1)).await.is_none());
}

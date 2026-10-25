//! The forager as a DTN node meets it: bundles that arrive twice, late,
//! in pieces, corrupted, all at once, or that are not asks at all, and
//! replies that must find their way home through a router.

mod common;

use std::time::Duration;

use bundle_protocol::administrative_record::{new_status_report_bundle, DELIVERED_BUNDLE, NO_INFORMATION};
use bundle_protocol::flags::BundleControlFlags;
use common::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn a_bundle_is_answered() {
    let f = Forager::start("plain", &[]).await;
    let node = Node::new("plain").await;
    node.send(&mut ask(&node.eid, &f.eid, b"hello from the dark"), f.addr).await;
    let reply = node.reply(WAIT).await.expect("an answer comes home");
    assert_eq!(reply.text, "echo: hello from the dark");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_json_ask_names_its_model_and_system() {
    let f = Forager::start("json", &[]).await;
    let node = Node::new("json").await;
    let body = json!({"text": "json-ask", "modelId": MODEL, "system": "be brief"});
    node.send(&mut ask(&node.eid, &f.eid, body.to_string().as_bytes()), f.addr).await;
    let reply = node.reply(WAIT).await.expect("an answer comes home");
    assert_eq!(reply.text, "echo: json-ask");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reply_goes_to_the_route_not_to_the_send_port() {
    let router = Node::new("router").await;
    let route = router.addr().to_string();
    let f = Forager::start("routed", &[("BP7_ROUTE", &route)]).await;
    let sender = Node::new("sender").await;
    sender.send(&mut ask(&sender.eid, &f.eid, b"route-me"), f.addr).await;
    let reply = router.reply(WAIT).await.expect("the reply reaches the router");
    assert_eq!(reply.text, "echo: route-me");
    assert_eq!(reply.first().primary.destination.to_string(), sender.eid, "addressed to the asker, through the router");
    assert!(sender.reply(QUIET).await.is_none(), "nothing goes back to the ephemeral port it was sent from");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bundle_heard_twice_is_answered_once() {
    let f = Forager::start("twice", &[]).await;
    let node = Node::new("twice").await;
    let mut b = ask(&node.eid, &f.eid, b"twice-marker");
    let bytes = b.to_cbor();
    node.send_raw(&bytes, f.addr).await;
    node.send_raw(&bytes, f.addr).await;
    let replies = node.replies(2, WAIT.min(Duration::from_secs(5))).await;
    assert_eq!(replies.len(), 1, "one answer for one bundle");
    assert_eq!(world().heard("twice-marker").len(), 1, "one bloom for one bundle");
    node.send_raw(&bytes, f.addr).await;
    assert!(node.reply(QUIET).await.is_none(), "a duplicate after the answer is not answered again");
    assert_eq!(world().heard("twice-marker").len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_burst_is_answered_each_to_its_own() {
    let f = Forager::start("burst", &[]).await;
    let node = Node::new("burst").await;
    let n = 16;
    for i in 0..n {
        node.send(&mut ask(&node.eid, &f.eid, format!("burst-{i}").as_bytes()), f.addr).await;
    }
    let replies = node.replies(n, WAIT).await;
    let mut got: Vec<String> = replies.iter().map(|r| r.text.clone()).collect();
    got.sort();
    let mut want: Vec<String> = (0..n).map(|i| format!("echo: burst-{i}")).collect();
    want.sort();
    assert_eq!(got, want, "every ask gets its own answer, and only its own");
    for r in &replies {
        let asked = r.text.trim_start_matches("echo: ");
        let block = hum_block(r.first()).expect("the reply says what it answers");
        assert!(block["to"].as_str().is_some_and(|id| !id.is_empty()), "{block}");
        let bloomed = world().heard(asked).iter().filter(|h| h.text == asked).count();
        assert_eq!(bloomed, 1, "{asked} bloomed once");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn blooms_are_bounded() {
    let f = Forager::start("bounded", &[("BP7_BLOOMS", "2")]).await;
    let node = Node::new("bounded").await;
    for i in 0..6 {
        node.send(&mut ask(&node.eid, &f.eid, format!("slow 300 bounded-{i}").as_bytes()), f.addr).await;
    }
    let replies = node.replies(6, WAIT).await;
    assert_eq!(replies.len(), 6);
    let peak = world().peak("bounded");
    assert!((1..=2).contains(&peak), "at most BP7_BLOOMS blooms at once, saw {peak}");
    assert!(
        world().heard("bounded-").len() == 6,
        "every ask bloomed once: {:?}",
        world().heard("bounded-")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_expired_bundle_is_not_answered() {
    let f = Forager::start("expired", &[]).await;
    let node = Node::new("expired").await;
    let mut old = ask_with(&node.eid, &f.eid, b"expired-marker", Duration::from_secs(60), dtn_ms_ago(3_600_000));
    node.send(&mut old, f.addr).await;
    node.send(&mut ask(&node.eid, &f.eid, b"expired-sentinel"), f.addr).await;
    let reply = node.reply(WAIT).await.expect("the live ask is answered");
    assert_eq!(reply.text, "echo: expired-sentinel");
    assert!(node.reply(QUIET).await.is_none());
    assert!(world().heard("expired-marker").is_empty(), "a bundle past its lifetime is not bloomed");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_status_report_is_not_an_ask() {
    let f = Forager::start("report", &[]).await;
    let node = Node::new("report").await;
    let mut ours = ask(&f.eid, &node.eid, b"report-marker");
    ours.primary.report_to = eid(&f.eid);
    let mut report = new_status_report_bundle(
        &ours,
        eid(&node.eid),
        bundle_protocol::crc::CRC_32,
        DELIVERED_BUNDLE,
        NO_INFORMATION,
    );
    node.send(&mut report, f.addr).await;
    node.send(&mut ask(&node.eid, &f.eid, b"report-sentinel"), f.addr).await;
    assert_eq!(node.reply(WAIT).await.expect("the ask is answered").text, "echo: report-sentinel");
    assert!(node.reply(QUIET).await.is_none(), "a status report is never answered");
    assert_eq!(world().heard("report-sentinel").len(), 1);
    assert!(
        world().heard("").iter().all(|h| !h.text.contains('\u{fffd}')),
        "no administrative record reached a nurse as words"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reply_is_never_answered() {
    let f = Forager::start("loop", &[]).await;
    let node = Node::new("loop").await;
    let mut reply = with_hum_block(
        ask(&node.eid, &f.eid, b"loop-marker: an answer from another hum"),
        json!({"hum": "reply", "to": "dtn://elsewhere/-1-0", "ok": true}),
    );
    node.send(&mut reply, f.addr).await;
    node.send(&mut ask(&node.eid, &f.eid, b"loop-sentinel"), f.addr).await;
    assert_eq!(node.reply(WAIT).await.expect("the ask is answered").text, "echo: loop-sentinel");
    assert!(node.reply(QUIET).await.is_none(), "two hums never answer each other's answers");
    assert!(world().heard("loop-marker").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_reply_is_a_bundle_any_dtn_node_accepts() {
    let f = Forager::start("conform", &[]).await;
    let node = Node::new("conform").await;
    let mut asked = ask_with(&node.eid, &f.eid, b"conform-marker", Duration::from_secs(86_400), bundle_protocol::CreationTimestamp::now());
    let asked_id = asked.id();
    node.send(&mut asked, f.addr).await;
    let reply = node.reply(WAIT).await.expect("an answer comes home");
    let mut b = reply.first().clone();
    assert!(b.validate().is_ok(), "{:?}", b.validate());
    assert!(b.primary.crc.has_crc(), "RFC 9171 4.3.1: a primary block without a BIB carries a CRC");
    assert!(b.crc_valid());
    assert_eq!(b.primary.source.to_string(), f.eid);
    assert_eq!(b.primary.destination.to_string(), node.eid);
    assert!(b.primary.lifetime >= Duration::from_secs(86_400), "the answer lives as long as the ask was given");
    let flags = b.primary.bundle_control_flags;
    for asked_for in [
        BundleControlFlags::BUNDLE_STATUS_REQUEST_RECEPTION,
        BundleControlFlags::BUNDLE_STATUS_REQUEST_FORWARD,
        BundleControlFlags::BUNDLE_STATUS_REQUEST_DELIVERY,
        BundleControlFlags::BUNDLE_STATUS_REQUEST_DELETION,
    ] {
        assert_eq!(flags & asked_for.bits(), 0, "a reply asks for no reports, so none can loop back as an ask");
    }
    assert!(!b.is_administrative_record());
    let block = hum_block(&b).expect("the reply is marked as hum's answer");
    assert_eq!(block["hum"], "reply");
    assert_eq!(block["to"], asked_id);
    assert_eq!(block["ok"], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_long_answer_is_fragmented_to_the_mtu() {
    let f = Forager::start("long", &[("BP7_MTU", "1200")]).await;
    let node = Node::new("long").await;
    node.send(&mut ask(&node.eid, &f.eid, b"big 100000"), f.addr).await;
    let reply = node.reply(WAIT).await.expect("a long answer still comes home");
    assert_eq!(reply.text.len(), 100_000);
    assert!(reply.text.bytes().all(|c| c == b'x'));
    assert!(reply.bundles.len() > 1, "fragmented");
    assert!(reply.sizes.iter().all(|s| *s <= 1200), "every datagram fits: {:?}", reply.sizes);
    for b in &reply.bundles {
        let mut b = b.clone();
        assert!(b.crc_valid() && b.validate().is_ok());
        assert!(hum_block(&b).is_some(), "the mark rides in every fragment");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fragmented_ask_is_reassembled() {
    let f = Forager::start("pieces", &[]).await;
    let node = Node::new("pieces").await;
    let text = "pieces-marker: ".to_string() + &"a question long enough to be cut ".repeat(20);
    let mut frags = fragments(&node.eid, &f.eid, text.as_bytes(), &[100, 400]);
    for i in [2, 0, 0, 1] {
        node.send(&mut frags[i], f.addr).await;
    }
    let reply = node.reply(WAIT).await.expect("the whole ask is answered");
    assert_eq!(reply.text, format!("echo: {text}"));
    let heard = world().heard("pieces-marker");
    assert_eq!(heard.len(), 1, "one bloom for the whole ask, not one per piece");
    assert_eq!(heard[0].text, text);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_corrupt_bundle_is_dropped() {
    let f = Forager::start("corrupt", &[]).await;
    let node = Node::new("corrupt").await;
    let mut bytes = ask(&node.eid, &f.eid, b"corrupt-marker: intact").to_cbor();
    let at = bytes.windows(6).position(|w| w == b"intact").unwrap();
    bytes[at] = b'I';
    node.send_raw(&bytes, f.addr).await;
    node.send(&mut ask(&node.eid, &f.eid, b"corrupt-sentinel"), f.addr).await;
    assert_eq!(node.reply(WAIT).await.expect("the clean ask is answered").text, "echo: corrupt-sentinel");
    assert!(node.reply(QUIET).await.is_none());
    assert!(world().heard("corrupt-marker").is_empty(), "a bundle whose CRC fails is not bloomed");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_anonymous_bundle_is_not_bloomed() {
    let f = Forager::start("anon", &[]).await;
    let node = Node::new("anon").await;
    node.send(&mut ask("dtn:none", &f.eid, b"anon-marker"), f.addr).await;
    node.send(&mut ask(&node.eid, &f.eid, b"anon-sentinel"), f.addr).await;
    assert_eq!(node.reply(WAIT).await.expect("the signed-off ask is answered").text, "echo: anon-sentinel");
    assert!(world().heard("anon-marker").is_empty(), "no one could hear the answer, so no bloom grows it");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bundle_for_another_endpoint_is_not_ours() {
    let f = Forager::start("other", &[]).await;
    let node = Node::new("other").await;
    let near = format!("{}-evil", f.eid);
    node.send(&mut ask(&node.eid, &near, b"other-marker"), f.addr).await;
    node.send(&mut ask(&node.eid, &f.eid, b"other-sentinel"), f.addr).await;
    assert_eq!(node.reply(WAIT).await.expect("ours is answered").text, "echo: other-sentinel");
    assert!(world().heard("other-marker").is_empty(), "{near} is not {}", f.eid);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_broken_bloom_is_answered_with_what_broke() {
    let f = Forager::start("broken", &[]).await;
    let node = Node::new("broken").await;
    node.send(&mut ask(&node.eid, &f.eid, b"fail"), f.addr).await;
    let reply = node.reply(WAIT).await.expect("the asker hears that it broke");
    assert!(reply.text.contains("the cell broke"), "{}", reply.text);
    let block = hum_block(reply.first()).expect("marked");
    assert_eq!(block["ok"], false);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ask_waits_for_a_nurse_to_come() {
    let f = Forager::start("later", &[]).await;
    let node = Node::new("later").await;
    let body = json!({"text": "later-marker", "modelId": "later"});
    node.send(&mut ask(&node.eid, &f.eid, body.to_string().as_bytes()), f.addr).await;
    assert!(node.reply(Duration::from_secs(2)).await.is_none(), "no nurse grows `later` yet");
    world().nurse("nurse-later", "later");
    let reply = node.reply(WAIT).await.expect("answered once a nurse comes");
    assert_eq!(reply.text, "echo: later-marker");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ask_outlives_the_forager() {
    let f = Forager::start("phoenix", &[]).await;
    let node = Node::new("phoenix").await;
    node.send(&mut ask(&node.eid, &f.eid, b"slow 2000 phoenix-marker"), f.addr).await;
    let since = std::time::Instant::now();
    while world().heard("phoenix-marker").is_empty() {
        assert!(since.elapsed() < WAIT, "the nurse never heard it");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let f = f.restart().await;
    let reply = node.reply(WAIT).await.expect("the ask survived the forager's restart");
    assert_eq!(reply.text, "echo: phoenix-marker");
    assert!(node.reply(Duration::from_secs(3)).await.is_none(), "and is answered once");
    drop(f);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_spool_forgets_what_has_expired() {
    let f = Forager::start("forget", &[("BP7_SWEEP_MS", "200")]).await;
    let node = Node::new("forget").await;
    let mut b = ask_with(&node.eid, &f.eid, b"forget-marker", Duration::from_secs(2), bundle_protocol::CreationTimestamp::now());
    node.send(&mut b, f.addr).await;
    assert_eq!(node.reply(WAIT).await.expect("answered").text, "echo: forget-marker");
    assert!(spool_files(&f.spool) > 0, "the answer is remembered while a duplicate could still arrive");
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(spool_files(&f.spool), 0, "and forgotten once no duplicate can");
}

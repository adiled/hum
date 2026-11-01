//! A sid as humd holds it: only the connections holding it hear its tones,
//! its cleanup or its forager's going frees what it held, and a peer that
//! leaves breaks off the blooms it was growing. A tendril an attendant
//! answers with an `error` comes back to its reacher as a failed grasp.

mod common;

use common::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn a_bloom_breaks_off_when_the_peer_growing_it_leaves() {
    let Ensemble2 { near, far, forager: (f, mut f_rx), nurse: (_, mut n_rx) } = forager_near_nurse_far().await;
    near.say(&f, prompt("gone")).await;
    hear(&mut n_rx, "prompt", "gone").await;

    near.ens.remove_peer(&far.ens.me());
    let error = hear(&mut f_rx, "error", "gone").await;
    assert_eq!(error["code"], "nurse-gone", "a forager grows it again on this code: {error}");
    assert!(near.thrum.wire_strays().is_empty(), "{:?}", near.thrum.wire_strays());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bloom_that_wilted_is_not_broken_off() {
    let Ensemble2 { near, far, forager: (f, mut f_rx), nurse: (n, mut n_rx) } = forager_near_nurse_far().await;
    near.say(&f, prompt("grown")).await;
    hear(&mut n_rx, "prompt", "grown").await;
    far.say(&n, wilt("grown")).await;
    hear(&mut f_rx, "wilt", "grown").await;

    near.ens.remove_peer(&far.ens.me());
    let late = heard_within(&mut f_rx, "error", "grown", QUIET).await;
    assert!(late.is_none(), "a bloom that wilted was broken off: {late:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bloom_grown_for_a_peer_reaches_no_bee_of_the_humd_growing_it() {
    let Ensemble2 { near, far, forager: (f, mut f_rx), nurse: (n, mut n_rx) } = forager_near_nurse_far().await;
    let (_, mut idle_rx) = far.forager().await;
    near.say(&f, prompt("theirs")).await;
    hear(&mut n_rx, "prompt", "theirs").await;
    far.say(&n, unfurl("theirs")).await;
    hear(&mut f_rx, "unfurl", "theirs").await;

    let leaked = heard_within(&mut idle_rx, "unfurl", "theirs", QUIET).await;
    assert!(leaked.is_none(), "a bee that never prompted it heard another bee's bloom: {leaked:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cleanup_follows_a_wilted_bloom_to_its_grower_and_frees_the_sid_there() {
    let Ensemble2 { near, far, forager: (f, mut f_rx), nurse: (n, mut n_rx) } = forager_near_nurse_far().await;
    near.say(&f, prompt("done")).await;
    hear(&mut n_rx, "prompt", "done").await;
    assert_eq!(far.wane_of("done"), Some(1));
    far.say(&n, wilt("done")).await;
    hear(&mut f_rx, "wilt", "done").await;

    near.say(&f, json!({"chi": "cleanup", "sid": "done"})).await;
    hear(&mut n_rx, "cleanup", "done").await;
    assert_eq!(far.wane_of("done"), None, "the humd growing it still holds the sid");
}

#[tokio::test(flavor = "multi_thread")]
async fn tones_for_a_sid_reach_no_bee_once_its_forager_has_gone() {
    let humd = Humd::start().await;
    let (n, mut n_rx) = humd.nurse().await;
    let (a, _a_rx) = humd.forager().await;
    let (_, mut b_rx) = humd.forager().await;
    humd.say(&a, prompt("orphan")).await;
    hear(&mut n_rx, "prompt", "orphan").await;

    humd.thrum.drop_synthetic(&a).await;
    humd.say(&n, unfurl("orphan")).await;
    let leaked = heard_within(&mut b_rx, "unfurl", "orphan", QUIET).await;
    assert!(leaked.is_none(), "another bee heard a bloom it never prompted: {leaked:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_nurse_reaching_a_tendril_does_not_hear_its_bloom_back() {
    let humd = Humd::start().await;
    let (n, mut n_rx) = humd.nurse().await;
    let (a, mut a_rx) = humd.forager().await;
    humd.say(&a, prompt("reach")).await;
    hear(&mut n_rx, "prompt", "reach").await;

    humd.say(&n, json!({"chi": "tendril-reach", "sid": "reach", "callId": "c1", "name": "nothing", "args": {}})).await;
    humd.say(&n, unfurl("reach")).await;
    hear(&mut a_rx, "unfurl", "reach").await;
    let echoed = heard_within(&mut n_rx, "unfurl", "reach", QUIET).await;
    assert!(echoed.is_none(), "the nurse heard its own bloom back: {echoed:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cleanup_frees_what_the_sid_held() {
    let humd = Humd::start().await;
    let (n, mut n_rx) = humd.nurse().await;
    let (a, mut a_rx) = humd.forager().await;
    humd.say(&a, prompt("ask")).await;
    hear(&mut n_rx, "prompt", "ask").await;
    assert_eq!(humd.wane_of("ask"), Some(1));
    humd.say(&n, wilt("ask")).await;
    hear(&mut a_rx, "wilt", "ask").await;

    assert!(humd.thrum.holds("ask"));
    humd.say(&a, json!({"chi": "cleanup", "sid": "ask"})).await;
    hear(&mut n_rx, "cleanup", "ask").await;
    assert!(!humd.thrum.holds("ask"), "the forager's connection still holds a sid it cleaned up");
    assert_eq!(humd.wane_of("ask"), None, "the sid's wane outlived its cleanup");
    humd.say(&n, unfurl("ask")).await;
    let late = heard_within(&mut a_rx, "unfurl", "ask", QUIET).await;
    assert!(late.is_none(), "the forager still holds a sid it cleaned up: {late:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sid_lasts_as_long_as_a_connection_holds_it() {
    let humd = Humd::start().await;
    let (n, mut n_rx) = humd.nurse().await;
    let (a, _a_rx) = humd.forager().await;
    let (b, _b_rx) = humd.forager().await;
    humd.say(&a, prompt("held")).await;
    hear(&mut n_rx, "prompt", "held").await;
    humd.say(&n, json!({"chi": "tendril-reach", "sid": "held", "callId": "c2", "name": "nothing", "args": {}})).await;
    humd.say(&b, json!({"chi": "attach", "sid": "held", "hearOnly": true})).await;

    humd.thrum.drop_synthetic(&a).await;
    assert_eq!(humd.wane_of("held"), Some(1), "freed while another connection still held it");
    humd.thrum.drop_synthetic(&b).await;
    assert_eq!(humd.wane_of("held"), None, "the sid outlived every connection that held it");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_closed_connection_releases_every_sid_it_held() {
    let humd = Humd::start().await;
    let (_, mut n_rx) = humd.nurse().await;
    let (a, _a_rx) = humd.forager().await;
    for sid in ["one", "two"] {
        humd.say(&a, prompt(sid)).await;
        hear(&mut n_rx, "prompt", sid).await;
    }
    humd.say(&a, json!({"chi": "attach", "sid": "three", "hearOnly": true})).await;
    assert!(["one", "two", "three"].iter().all(|sid| humd.thrum.holds(sid)));

    humd.thrum.drop_synthetic(&a).await;
    for sid in ["one", "two", "three"] {
        assert!(!humd.thrum.holds(sid), "{sid} is still held");
        assert_eq!(humd.wane_of(sid), None, "{sid} outlived the connection that held it");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_attendants_error_comes_back_to_the_nurse_as_a_failed_grasp() {
    let humd = Humd::start().await;
    let (n, mut n_rx) = humd.nurse().await;
    let (a, mut a_rx) = humd.attendant().await;
    let (f, _f_rx) = humd.forager().await;
    humd.say(&f, prompt("tool")).await;
    hear(&mut n_rx, "prompt", "tool").await;

    humd.say(&n, reach("tool", "c3")).await;
    answer_with_error(&humd, &a, &mut a_rx, "c3").await;
    let failed = grasp(&mut n_rx, "c3", WAIT).await.expect("the nurse never heard how its reach ended");
    assert_eq!(failed["isError"], true);
    assert_eq!(failed["output"], "no such thing");
    assert_eq!(failed["sid"], "tool");
    assert_eq!(failed["result"]["code"], "402", "the error's code is carried");
    assert_eq!(failed["ext"]["x402"]["price"], "1", "the error's ext is carried whole");

    humd.say(&a, json!({"chi": "tendril-result", "sid": "tool", "callId": "c3", "output": "late"})).await;
    let late = grasp(&mut n_rx, "c3", QUIET).await;
    assert!(late.is_none(), "the reach's route outlived its answer: {late:?}");
    let sent: Vec<String> = humd.thrum.wire_strays().into_iter().filter(|s| s.starts_with("sent")).collect();
    assert!(sent.is_empty(), "humd sounded what the thrum does not declare: {sent:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_peer_attendants_error_comes_back_to_the_nurse_as_a_failed_grasp() {
    let here = Humd::start().await;
    let there = Humd::start().await;
    wire(&here, &there);
    let (a, mut a_rx) = there.attendant().await;
    let (n, mut n_rx) = here.nurse().await;
    let (f, mut f_rx) = here.forager().await;
    reaches(&mut f_rx, "tools", TOOL).await;
    here.say(&f, prompt("far-tool")).await;
    hear(&mut n_rx, "prompt", "far-tool").await;

    here.say(&n, reach("far-tool", "c4")).await;
    answer_with_error(&there, &a, &mut a_rx, "c4").await;
    let failed = grasp(&mut n_rx, "c4", WAIT).await.expect("the nurse never heard how its reach ended");
    assert_eq!(failed["isError"], true);
    let broke = heard_within(&mut f_rx, "error", "far-tool", QUIET).await;
    assert!(broke.is_none(), "a failed grasp broke off the forager's bloom: {broke:?}");

    there.say(&a, json!({"chi": "tendril-result", "sid": "far-tool", "callId": "c4", "output": "late"})).await;
    let late = grasp(&mut n_rx, "c4", QUIET).await;
    assert!(late.is_none(), "the reach's route outlived its answer: {late:?}");
}

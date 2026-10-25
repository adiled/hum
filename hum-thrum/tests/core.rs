//! The core, fed by hand: what humd does on the wire goes in, and what
//! the bee would see and what the driver would do come out.

use std::time::Duration;

use hum_identity::BeeKey;
use hum_thrum::{Advisory, Core, Endpoint, Output, Timer, BACKOFF_MOST, VERDICT};
use serde_json::{Map, Value, json};

const NOW: i64 = 1_793_000_000_000;

fn key() -> BeeKey {
    BeeKey::from_seed(&[7u8; 32])
}

fn core_with(hello: Value) -> Core {
    let Value::Object(hello) = hello else { panic!() };
    Core::new("dwarfstar", thrum_core::Role::Nurse, key(), hello, Endpoint::Resolve)
}

fn drain(core: &mut Core) -> Vec<Output> {
    std::iter::from_fn(|| core.poll()).collect()
}

fn written(outs: &[Output]) -> Vec<Value> {
    outs.iter()
        .filter_map(|o| match o {
            Output::Write(bytes) => {
                assert_eq!(bytes.last(), Some(&b'\n'), "a tone is one line");
                Some(serde_json::from_slice(&bytes[..bytes.len() - 1]).unwrap())
            }
            _ => None,
        })
        .collect()
}

fn advisories(outs: &[Output]) -> Vec<Advisory> {
    outs.iter()
        .filter_map(|o| match o {
            Output::Advisory(a) => Some(a.clone()),
            _ => None,
        })
        .collect()
}

fn admitted() -> Vec<Output> {
    vec![Output::Nestled, Output::Tone(json!({"chi": "breath", "sigils": {}}))]
}

fn line(v: Value) -> Vec<u8> {
    let mut b = serde_json::to_vec(&v).unwrap();
    b.push(b'\n');
    b
}

fn greet(core: &mut Core) -> String {
    core.start();
    assert_eq!(drain(core), vec![Output::Connect(Endpoint::Resolve)]);
    core.connected(NOW);
    let outs = drain(core);
    let hello = written(&outs).remove(0);
    assert!(outs.contains(&Output::Arm(Timer::Verdict, VERDICT)));
    hello["rid"].as_str().unwrap().to_string()
}

fn nestle(core: &mut Core) -> String {
    let rid = greet(core);
    core.received(NOW, &line(json!({"chi": "breath", "sigils": []})));
    assert!(drain(core).is_empty(), "the breath on connect admits nothing");
    core.received(NOW, &line(json!({"chi": "echo", "rid": rid, "ok": true})));
    assert!(drain(core).is_empty(), "the acknowledgement is not the verdict");
    core.received(NOW, &line(json!({"chi": "breath", "sigils": {}})));
    assert_eq!(drain(core), admitted(), "the breath that admits is the bee's too");
    assert!(core.is_nestled());
    rid
}

#[test]
fn the_hello_is_signed_and_carries_what_the_client_owns() {
    let mut core = core_with(json!({"role": "forager", "models": ["ds4flash"]}));
    core.start();
    drain(&mut core);
    core.connected(NOW);
    let hello = written(&drain(&mut core)).remove(0);
    let key = key();
    let sig = key.sign_socket_hello(NOW);
    assert_eq!(hello["chi"], "hello");
    assert_eq!(hello["hive"], "dwarfstar");
    assert_eq!(hello["role"], "nurse", "the client owns the role it entered for");
    assert_eq!(hello["protoVersion"], thrum_core::THRUM_VERSION);
    assert_eq!(hello["hid"], key.id());
    assert_eq!(hello["pubkey"], sig.pubkey);
    assert_eq!(hello["signedAt"], NOW);
    assert_eq!(hello["signature"], sig.signature);
    assert_eq!(hello["models"], json!(["ds4flash"]));
    assert!(hum_identity::is_valid_id(hello["rid"].as_str().unwrap()));
    assert!(thrum_core::undeclared(&hello).is_none(), "the client's hello is the spec's: {hello}");
}

#[test]
fn a_breath_after_the_acknowledgement_admits() {
    let mut core = core_with(json!({}));
    nestle(&mut core);
}

#[test]
fn a_bee_split_across_reads_still_hears_whole_tones() {
    let mut core = core_with(json!({}));
    let rid = greet(&mut core);
    let mut bytes = line(json!({"chi": "echo", "rid": rid, "ok": true}));
    bytes.extend(line(json!({"chi": "breath", "sigils": {}})));
    let (a, b) = bytes.split_at(7);
    core.received(NOW, a);
    assert!(drain(&mut core).is_empty());
    core.received(NOW, b);
    assert_eq!(drain(&mut core), admitted());
}

#[test]
fn a_refusal_is_final() {
    let mut core = core_with(json!({}));
    let rid = greet(&mut core);
    core.received(NOW, &line(json!({"chi": "echo", "rid": rid, "ok": true})));
    core.received(NOW, &line(json!({"chi": "echo", "rid": rid, "ok": false, "error": "bee identity revoked"})));
    assert_eq!(
        drain(&mut core),
        vec![Output::Close, Output::Refused("bee identity revoked".into())]
    );
    core.closed(NOW);
    core.fired(NOW, Timer::Retry);
    core.send(NOW, json!({"chi": "unfurl", "sid": "s"}));
    assert!(drain(&mut core).is_empty(), "a refused bee does nothing more");
}

#[test]
fn a_refusal_after_admission_is_still_a_refusal() {
    let mut core = core_with(json!({}));
    let rid = nestle(&mut core);
    core.received(NOW, &line(json!({"chi": "echo", "rid": rid, "ok": false, "error": "no"})));
    assert_eq!(drain(&mut core), vec![Output::Close, Output::Refused("no".into())]);
}

#[test]
fn a_bee_outlives_humd_and_backs_off() {
    let mut core = core_with(json!({}));
    nestle(&mut core);
    core.closed(NOW);
    let outs = drain(&mut core);
    assert_eq!(outs[0], Output::Astray);
    let Output::Arm(Timer::Retry, first) = outs[1] else { panic!("{outs:?}") };
    assert!((200..=300).contains(&first.as_millis()), "{first:?}");

    core.fired(NOW, Timer::Retry);
    assert_eq!(drain(&mut core), vec![Output::Connect(Endpoint::Resolve)]);
    core.closed(NOW);
    let Output::Arm(Timer::Retry, second) = drain(&mut core)[0] else { panic!() };
    assert!((400..=600).contains(&second.as_millis()), "{second:?}");

    core.fired(NOW, Timer::Retry);
    drain(&mut core);
    nestle_again(&mut core);
    core.closed(NOW);
    let outs = drain(&mut core);
    let Output::Arm(Timer::Retry, reset) = outs[1] else { panic!() };
    assert!(reset.as_millis() <= 300, "admission resets the wait: {reset:?}");
}

fn nestle_again(core: &mut Core) {
    core.connected(NOW);
    let hello = written(&drain(core)).remove(0);
    let rid = hello["rid"].as_str().unwrap().to_string();
    core.received(NOW, &line(json!({"chi": "echo", "rid": rid, "ok": true})));
    core.received(NOW, &line(json!({"chi": "breath", "sigils": {}})));
    assert_eq!(drain(core), admitted(), "the breath that admits is the bee's too");
}

#[test]
fn the_wait_stops_growing_at_its_ceiling() {
    for tries in [12, 20, 40] {
        let wait = hum_thrum::core::backoff(tries);
        assert!(wait <= BACKOFF_MOST.mul_f64(1.2), "{wait:?}");
        assert!(wait >= BACKOFF_MOST.mul_f64(0.8), "{wait:?}");
    }
}

#[test]
fn a_humd_with_no_verdict_is_gone() {
    let mut core = core_with(json!({}));
    greet(&mut core);
    core.fired(NOW, Timer::Verdict);
    let outs = drain(&mut core);
    assert_eq!(outs[0], Output::Close);
    assert!(matches!(outs[1], Output::Arm(Timer::Retry, _)));
}

#[test]
fn what_a_bee_sends_gets_a_rid_and_the_ack_of_it_is_swallowed() {
    let mut core = core_with(json!({}));
    nestle(&mut core);
    core.send(NOW, json!({"chi": "unfurl", "sid": "s", "petal": 0, "kind": "text", "text": "hi"}));
    let tone = written(&drain(&mut core)).remove(0);
    let rid = tone["rid"].as_str().unwrap().to_string();
    assert!(hum_identity::is_valid_id(&rid));

    core.received(NOW, &line(json!({"chi": "echo", "rid": rid, "ok": true})));
    core.received(NOW, &line(json!({"chi": "echo", "rid": rid, "ok": true})));
    assert!(drain(&mut core).is_empty(), "an ack of a minted rid is not the bee's, however often it comes");

    core.send(NOW, json!({"chi": "unfurl", "sid": "s", "petal": 0, "kind": "text", "text": "x"}));
    let rid = written(&drain(&mut core))[0]["rid"].as_str().unwrap().to_string();
    core.received(NOW, &line(json!({"chi": "echo", "rid": rid, "ok": false, "error": "past dusk"})));
    let advs = advisories(&drain(&mut core));
    assert_eq!(advs[0].code, "tone-refused");

    core.send(NOW, json!({"chi": "prompt", "rid": "mine", "sid": "s"}));
    drain(&mut core);
    core.received(NOW, &line(json!({"chi": "echo", "rid": "mine", "ok": true})));
    assert_eq!(
        drain(&mut core),
        vec![Output::Tone(json!({"chi": "echo", "rid": "mine", "ok": true}))],
        "the ack of a rid the bee chose is the bee's"
    );
}

#[test]
fn an_undeclared_key_is_sent_with_one_advisory() {
    let mut core = core_with(json!({}));
    nestle(&mut core);
    let tone = json!({"chi": "unfurl", "sid": "s", "petal": 0, "kind": "text", "text": "a", "delta": "a"});
    core.send(NOW, tone.clone());
    let outs = drain(&mut core);
    assert_eq!(written(&outs).len(), 1, "sent anyway");
    let advs = advisories(&outs);
    assert_eq!(advs.len(), 1);
    assert_eq!(advs[0].code, "undeclared-key");
    assert_eq!(advs[0].from, "client");
    assert_eq!(advs[0].fields["keys"], json!(["delta"]));
    core.send(NOW, tone);
    assert!(advisories(&drain(&mut core)).is_empty(), "once per chi and key");

    core.send(NOW, json!({"chi": "unfurls", "sid": "s"}));
    let outs = drain(&mut core);
    assert_eq!(advisories(&outs)[0].code, "unknown-chi");
    assert_eq!(written(&outs).len(), 1);
}

#[test]
fn what_is_not_a_tone_is_dropped_with_an_advisory() {
    let mut core = core_with(json!({}));
    nestle(&mut core);
    core.send(NOW, json!(["unfurl"]));
    core.send(NOW, json!({"sid": "s"}));
    let outs = drain(&mut core);
    assert!(written(&outs).is_empty());
    assert_eq!(advisories(&outs).iter().filter(|a| a.code == "not-a-tone").count(), 2);
}

#[test]
fn what_is_sent_while_humd_is_away_is_dropped_and_counted() {
    let mut core = core_with(json!({}));
    nestle(&mut core);
    core.closed(NOW);
    drain(&mut core);
    core.send(NOW, json!({"chi": "unfurl", "sid": "s", "petal": 0, "kind": "text", "text": "a"}));
    core.send(NOW, json!({"chi": "wilt", "sid": "s", "reason": "stop"}));
    assert!(written(&drain(&mut core)).is_empty());
    core.fired(NOW, Timer::Retry);
    drain(&mut core);
    core.connected(NOW);
    let rid = written(&drain(&mut core))[0]["rid"].as_str().unwrap().to_string();
    core.received(NOW, &line(json!({"chi": "echo", "rid": rid, "ok": true})));
    core.received(NOW, &line(json!({"chi": "breath", "sigils": {}})));
    let outs = drain(&mut core);
    assert_eq!(outs[0], Output::Nestled);
    let advs = advisories(&outs);
    assert_eq!(advs[0].code, "dropped-while-away");
    assert_eq!(advs[0].fields["count"], 2);
}

#[test]
fn a_reconnect_announces_the_latest_headroom() {
    let mut core = core_with(json!({"headroom": {"freeSlots": 1, "totalSlots": 1}}));
    nestle(&mut core);
    core.send(NOW, json!({"chi": "pulse", "kind": "cell-ready", "headroom": {"freeSlots": 0, "totalSlots": 1}}));
    drain(&mut core);
    core.closed(NOW);
    core.fired(NOW, Timer::Retry);
    drain(&mut core);
    core.connected(NOW);
    let hello = written(&drain(&mut core)).remove(0);
    assert_eq!(hello["headroom"], json!({"freeSlots": 0, "totalSlots": 1}));
}

#[test]
fn a_changed_hello_goes_out_at_once_and_its_breath_is_heard() {
    let mut core = core_with(json!({"models": ["a"]}));
    nestle(&mut core);
    let mut patch = Map::new();
    patch.insert("models".into(), json!(["a", "b"]));
    patch.insert("hid".into(), json!("bee_forged"));
    core.hello(NOW, patch);
    let outs = drain(&mut core);
    let hello = written(&outs).remove(0);
    assert_eq!(hello["models"], json!(["a", "b"]));
    assert_eq!(hello["hid"], key().id(), "the client owns the hid");
    assert_eq!(advisories(&outs)[0].code, "client-owned");
    let rid = hello["rid"].as_str().unwrap().to_string();
    core.received(NOW, &line(json!({"chi": "echo", "rid": rid, "ok": true})));
    core.received(NOW, &line(json!({"chi": "breath", "sigils": {}})));
    assert_eq!(drain(&mut core).len(), 1, "the re-hello's breath is the bee's");
    core.received(NOW, &line(json!({"chi": "breath", "sigils": {}})));
    assert_eq!(drain(&mut core).len(), 1, "a later breath is the bee's");
    core.received(NOW, &line(json!({"chi": "echo", "rid": rid, "ok": false, "error": "revoked"})));
    assert_eq!(drain(&mut core)[1], Output::Refused("revoked".into()));
}

#[test]
fn a_hello_sent_as_a_tone_is_a_change_to_the_hello() {
    let mut core = core_with(json!({}));
    nestle(&mut core);
    core.send(NOW, json!({"chi": "hello", "models": ["m"]}));
    let outs = drain(&mut core);
    let hello = written(&outs).remove(0);
    assert_eq!(hello["models"], json!(["m"]));
    assert!(hello["signature"].is_string());
}

#[test]
fn humd_advises_through_a_log_tone() {
    let mut core = core_with(json!({}));
    nestle(&mut core);
    core.received(
        NOW,
        &line(json!({
            "chi": "log",
            "level": "warn",
            "message": "unfurl carries delta",
            "fields": {"advisory": "undeclared-key", "keys": ["delta"]},
        })),
    );
    let outs = drain(&mut core);
    assert_eq!(
        outs,
        vec![Output::Advisory(Advisory {
            code: "undeclared-key".into(),
            message: "unfurl carries delta".into(),
            from: "humd".into(),
            fields: serde_json::from_value(json!({"keys": ["delta"]})).unwrap(),
        })]
    );
    core.send(NOW, json!({"chi": "unfurl", "sid": "s", "petal": 0, "kind": "text", "text": "a", "x": 1}));
    drain(&mut core);
    core.received(
        NOW,
        &line(json!({
            "chi": "log",
            "level": "warn",
            "message": "unfurl carries x",
            "fields": {"advisory": "undeclared-key", "chi": "unfurl", "keys": ["x"]},
        })),
    );
    assert!(drain(&mut core).is_empty(), "humd agreeing with the client is not news");
    core.received(NOW, &line(json!({"chi": "log", "level": "info", "message": "plain"})));
    assert!(matches!(drain(&mut core)[0], Output::Tone(_)), "a plain log is a tone");
}

#[test]
fn stopping_closes_once() {
    let mut core = core_with(json!({}));
    nestle(&mut core);
    core.stop();
    assert_eq!(drain(&mut core), vec![Output::Close, Output::Stopped]);
    core.stop();
    assert!(drain(&mut core).is_empty());
}

#[test]
fn a_never_admitted_bee_retries_without_lost() {
    let mut core = core_with(json!({}));
    core.start();
    drain(&mut core);
    core.closed(NOW);
    let outs = drain(&mut core);
    assert_eq!(outs.len(), 1);
    assert!(matches!(outs[0], Output::Arm(Timer::Retry, d) if d < Duration::from_secs(1)));
}

#[test]
fn a_hello_changed_before_the_verdict_goes_out_once_admitted() {
    let mut core = core_with(json!({"models": ["a"]}));
    let rid = greet(&mut core);
    let mut patch = Map::new();
    patch.insert("models".into(), json!(["a", "b"]));
    core.hello(NOW, patch);
    assert!(drain(&mut core).iter().all(|o| !matches!(o, Output::Write(_))), "not before the verdict");
    core.received(NOW, &line(json!({"chi": "echo", "rid": rid, "ok": true})));
    core.received(NOW, &line(json!({"chi": "breath", "sigils": {}})));
    let outs = drain(&mut core);
    assert_eq!(outs[0], Output::Nestled);
    let hello = written(&outs).remove(0);
    assert_eq!(hello["chi"], "hello");
    assert_eq!(hello["models"], json!(["a", "b"]));
}

#[test]
fn headroom_claimed_while_humd_is_away_is_announced_on_return() {
    let mut core = core_with(json!({"headroom": {"freeSlots": 1, "totalSlots": 1}}));
    nestle(&mut core);
    core.closed(NOW);
    drain(&mut core);
    core.send(NOW, json!({"chi": "pulse", "kind": "cell-ready", "headroom": {"freeSlots": 0, "totalSlots": 1}}));
    core.fired(NOW, Timer::Retry);
    drain(&mut core);
    core.connected(NOW);
    let hello = written(&drain(&mut core)).remove(0);
    assert_eq!(hello["headroom"], json!({"freeSlots": 0, "totalSlots": 1}));
}

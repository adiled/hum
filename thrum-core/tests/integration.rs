//! Integration tests for `thrum-core`.
//!
//! These exercise the public API surface — sigil determinism, chi
//! round-trip across the full registry, envelope rid contract, dusk
//! semantics, WaneTracker drift detection, and the protocol version
//! pin. They are the contract a downstream crate can rely on.

use serde_json::{json, Value};
use thrum_core::{
    dusk_in, is_dusk, sigil, Chi, Envelope, Tone, WaneTracker, THRUM_VERSION,
};


#[test]
fn sigil_is_deterministic() {
    assert_eq!(sigil("session-abc"), sigil("session-abc"), "same session, same sigil");
}

#[test]
fn sigil_differs_by_sid() {
    assert_ne!(sigil("session-1"), sigil("session-2"));
}

#[test]
fn sigil_is_12_hex_chars() {
    let s = sigil("anything");
    assert_eq!(s.len(), 12);
    assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(s.chars().all(|c| !c.is_ascii_uppercase()));
}


fn assert_chi_round_trips(chi: Chi) {
    let tone = Tone::new(Envelope::new(chi, "rid-rt"));
    let s = serde_json::to_string(&tone).expect("serialize tone");
    let back: Tone = serde_json::from_str(&s).expect("parse tone");
    assert_eq!(back.chi(), chi, "chi mismatch after round-trip: {chi:?}");
    assert_eq!(back.rid(), "rid-rt");
}

#[test]
fn chi_hello_round_trips() {
    assert_chi_round_trips(Chi::Hello);
}

#[test]
fn chi_prompt_round_trips() {
    assert_chi_round_trips(Chi::Prompt);
}

#[test]
fn chi_cancel_round_trips() {
    assert_chi_round_trips(Chi::Cancel);
}

#[test]
fn chi_cleanup_round_trips() {
    assert_chi_round_trips(Chi::Cleanup);
}

#[test]
fn chi_curate_round_trips() {
    assert_chi_round_trips(Chi::Curate);
}

#[test]
fn chi_release_permit_round_trips() {
    assert_chi_round_trips(Chi::ReleasePermit);
}

#[test]
fn chi_scout_result_round_trips() {
    assert_chi_round_trips(Chi::ScoutResult);
}

#[test]
fn chi_tendril_result_round_trips() {
    assert_chi_round_trips(Chi::TendrilResult);
}

#[test]
fn chi_petal_cell_round_trips() {
    assert_chi_round_trips(Chi::PetalCell);
}

#[test]
fn chi_breath_round_trips() {
    assert_chi_round_trips(Chi::Breath);
}

#[test]
fn chi_unfurl_round_trips() {
    assert_chi_round_trips(Chi::Unfurl);
}

#[test]
fn chi_wilt_round_trips() {
    assert_chi_round_trips(Chi::Wilt);
}

#[test]
fn chi_error_round_trips() {
    assert_chi_round_trips(Chi::Error);
}

#[test]
fn chi_pulse_round_trips() {
    assert_chi_round_trips(Chi::Pulse);
}

#[test]
fn chi_permission_ask_round_trips() {
    assert_chi_round_trips(Chi::PermissionAsk);
}

#[test]
fn chi_scout_round_trips() {
    assert_chi_round_trips(Chi::Scout);
}

#[test]
fn chi_tendril_reach_round_trips() {
    assert_chi_round_trips(Chi::TendrilReach);
}

#[test]
fn chi_tendril_meta_round_trips() {
    assert_chi_round_trips(Chi::TendrilMeta);
}

#[test]
fn chi_echo_round_trips() {
    assert_chi_round_trips(Chi::Echo);
}

#[test]
fn chi_perf_mark_round_trips() {
    assert_chi_round_trips(Chi::PerfMark);
}

#[test]
fn chi_log_round_trips() {
    assert_chi_round_trips(Chi::Log);
}

#[test]
fn chi_drone_round_trips() {
    assert_chi_round_trips(Chi::Drone);
}

#[test]
fn chi_drone_retrofit_round_trips() {
    assert_chi_round_trips(Chi::DroneRetrofit);
}


#[test]
fn envelope_requires_rid_to_deserialize() {
    let wire = json!({ "chi": "hello" });
    let result: Result<Envelope, _> = serde_json::from_value(wire);
    assert!(
        result.is_err(),
        "envelope without rid must fail to deserialize, got {result:?}"
    );
}

#[test]
fn tone_requires_rid_to_deserialize() {
    let wire = json!({ "chi": "prompt", "content": "hi" });
    let result: Result<Tone, _> = serde_json::from_value(wire);
    assert!(
        result.is_err(),
        "tone without rid must fail to deserialize, got {result:?}"
    );
}

#[test]
fn envelope_with_rid_deserializes() {
    let wire = json!({ "chi": "hello", "rid": "r-1" });
    let env: Envelope = serde_json::from_value(wire).expect("rid present, must parse");
    assert_eq!(env.rid, "r-1");
    assert_eq!(env.chi, Chi::Hello);
}


#[test]
fn is_dusk_true_past_expiry() {
    let env = Envelope { dusk: Some(dusk_in(-1)), ..Envelope::new(Chi::Hello, "r") };
    assert!(is_dusk(&env), "expired dusk must register as dusk");
}

#[test]
fn is_dusk_false_before_expiry() {
    let env = Envelope { dusk: Some(dusk_in(10_000)), ..Envelope::new(Chi::Hello, "r") };
    assert!(!is_dusk(&env), "future dusk must not register as dusk");
}

#[test]
fn is_dusk_false_when_unset() {
    let env = Envelope::new(Chi::Hello, "r");
    assert!(!is_dusk(&env), "missing dusk must never register as dusk");
}


#[test]
fn wane_tick_increments() {
    let w = WaneTracker::new();
    assert_eq!(w.get("alpha"), 0);
    assert_eq!(w.tick("alpha"), 1);
    assert_eq!(w.tick("alpha"), 2);
    assert_eq!(w.tick("alpha"), 3);
    assert_eq!(w.get("alpha"), 3);
}

#[test]
fn wane_tick_is_per_sigil() {
    let w = WaneTracker::new();
    w.tick("alpha");
    w.tick("alpha");
    w.tick("beta");
    assert_eq!(w.get("alpha"), 2);
    assert_eq!(w.get("beta"), 1);
    assert_eq!(w.get("gamma"), 0);
}

#[test]
fn wane_behind_detects_remote_ahead() {
    let w = WaneTracker::new();
    w.tick("s");
    w.tick("s");
    assert!(w.behind("s", 5));
    assert!(w.behind("s", 3));
    assert!(!w.behind("s", 2));
    assert!(!w.behind("s", 1));
    assert!(!w.behind("s", 0));
}

#[test]
fn wane_behind_on_untracked_sigil() {
    let w = WaneTracker::new();
    assert!(w.behind("fresh", 1));
    assert!(!w.behind("fresh", 0));
}

#[test]
fn wane_set_pins_value() {
    let w = WaneTracker::new();
    w.set("s", 42);
    assert_eq!(w.get("s"), 42);
    assert_eq!(w.tick("s"), 43);
}


#[test]
fn thrum_version_is_pinned() {
    assert_eq!(THRUM_VERSION, "0.8.0");
}


#[test]
fn tone_with_body_round_trips_via_json_string() {
    let wire = json!({
        "chi": "tendril-reach",
        "rid": "abc-1",
        "sid": "session-x",
        "from": "humd",
        "name": "Read",
        "args": { "path": "/tmp/x" },
    });
    let s = serde_json::to_string(&wire).unwrap();
    let tone: Tone = serde_json::from_str(&s).expect("parse from string");
    assert_eq!(tone.chi(), Chi::TendrilReach);
    assert_eq!(tone.rid(), "abc-1");
    let back: Value = serde_json::from_str(&serde_json::to_string(&tone).unwrap()).unwrap();
    assert_eq!(back, wire);
}

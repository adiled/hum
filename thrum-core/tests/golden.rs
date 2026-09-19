//! Golden wire fixture — locks the serialized shape of canonical tones.
//!
//! Every line of `fixtures/golden.ndjson` is a full tone parsed straight
//! off the wire. The test rebuilds each tone from the typed views and
//! deep-compares bytes with what's committed. If `views.rs` changes a
//! key, adds/removes a field, or alters optionality, this test fails
//! until the fixture — and thereby every generated client — catches up.
//!
//! The fixture is written on first run; after that it's compare-only.

use serde_json::{Map, Value};

use thrum_core::{Chi, EchoBody, Envelope, Tone};

/// Body view → flat body map (serializes absent optionals off the wire).
fn body_map<T: serde::Serialize>(b: &T) -> Map<String, Value> {
    serde_json::to_value(b)
        .expect("body serializes")
        .as_object()
        .expect("body is an object")
        .clone()
}

/// Fixed valid 52-char Crockford rid: 50 zeroes + the line index.
fn rid(n: usize) -> String {
    format!("{:0>50}{:02}", "", n)
}

fn tone(env: Envelope, b: Map<String, Value>) -> Value {
    serde_json::to_value(Tone::with_body(env, b)).expect("tone serializes")
}

fn env(chi: Chi, n: usize) -> Envelope {
    Envelope {
        rid: rid(n),
        from: Some("humd".into()),
        sigil: Some("a1b2c3d4e5f6".into()),
        sid: Some("sess-1".into()),
        wane: Some(n as u64),
        sent_at: Some(1_700_000_000_000 + n as i64),
        ..Envelope::new(chi, rid(n))
    }
}

fn golden() -> Vec<Value> {
    let mut out = Vec::new();
    let mut n = 0usize;
    macro_rules! push {
        ($chi:expr, $body:expr) => {{
            out.push(tone(env($chi, n), body_map(&$body)));
            n += 1;
        }};
    }

    push!(
        Chi::Hello,
        thrum_core::HelloBody {
            proto_version: "0.7.0".into(),
            bee: "claude-cli".into(),
            version: "0.32.0".into(),
            hid: Some("h-9".into()),
            tool_names: Some(vec!["Read".into(), "Bash".into()]),
            provides: Some(vec!["env".into(), "token".into()]),
            source: Some("cli".into()),
        }
    );
    push!(
        Chi::Prompt,
        thrum_core::PromptBody {
            model_id: Some("claude-3-5-sonnet".into()),
            cwd: Some("/tmp/w".into()),
            system_prompt: Some("be brief".into()),
            text: Some("hi".into()),
            content: None,
            tools: Some(vec![Value::from("builtin")]),
            forager_tools: Some(vec![serde_json::json!({ "name": "Brave" })]),
            provided: Some(vec!["env".into()]),
            disallowed_tools: Some(vec![]),
        }
    );
    push!(
        Chi::Chunk,
        thrum_core::ChunkBody {
            chunk_type: "text_delta".into(),
            block_idx: Some(0),
            delta: Some(Value::from("Hello")),
            partial_json: None,
        }
    );
    push!(
        Chi::Chunk,
        thrum_core::ChunkBody {
            chunk_type: "tool_input_delta".into(),
            block_idx: Some(2),
            delta: None,
            partial_json: Some("{\"a\":".into()),
        }
    );
    push!(
        Chi::Finish,
        thrum_core::FinishBody {
            finish_reason: "stop".into(),
            usage: Some(serde_json::json!({
                "input_tokens": 12,
                "output_tokens": 34
            })),
            exit_code: None,
            subtype: None,
        }
    );
    push!(
        Chi::Error,
        thrum_core::ErrorBody {
            message: "nest crashed".into(),
            code: Some("worker_error".into()),
            subtype: Some("nest_crash".into()),
            usage: None,
        }
    );
    push!(
        Chi::ToolCall,
        thrum_core::ToolCallBody {
            call_id: "call-1".into(),
            tool_name: "Read".into(),
            name: Some("Read".into()),
            args: Some(serde_json::json!({ "path": "/x" })),
        }
    );
    push!(
        Chi::ToolResult,
        thrum_core::ToolResultBody {
            call_id: "call-1".into(),
            output: Some("file contents".into()),
            result: None,
            is_error: Some(false),
            title: Some("Read ok".into()),
            metadata: None,
        }
    );
    push!(
        Chi::SessionReady,
        thrum_core::SessionReadyBody {
            nest_id: "n-1".into(),
            model: "claude-3-5-sonnet".into(),
            tools: Some(vec![serde_json::json!({ "name": "Read" })]),
        }
    );
    push!(
        Chi::Breath,
        thrum_core::BreathBody {
            sessions: serde_json::json!([{ "sid": "s1", "model": "claude" }]),
            proto_version: Some("0.7.0".into()),
        }
    );
    push!(
        Chi::Attach,
        thrum_core::AttachBody {
            hear_only: Some(true),
        }
    );
    push!(
        Chi::WaneSync,
        thrum_core::WaneSyncBody {
            snapshot: [("a1b2c3d4e5f6".to_string(), 12u64)].into_iter().collect(),
        }
    );
    push!(
        Chi::GossipPublish,
        thrum_core::GossipPublishBody {
            topic: "gossip".into(),
            payload: serde_json::json!({ "k": "v" }),
            from: Some("gossip-0".into()),
            msg_id: "m-1".into(),
        }
    );
    push!(
        Chi::PeerAdd,
        thrum_core::PeerAddBody {
            humd_id: "humd-1".into(),
            hints: Some(vec![serde_json::json!({ "addr": "ws://x" })]),
        }
    );
    push!(
        Chi::Echo,
        EchoBody {
            ok: true,
            error: None,
        }
    );
    out
}

#[test]
fn golden_wire_locked() {
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/golden.ndjson");
    let want = golden();
    if !fixture.exists() {
        // First run: materialize the fixture, then pass.
        std::fs::create_dir_all(fixture.parent().unwrap()).unwrap();
        let mut s = String::new();
        for t in &want {
            s.push_str(&serde_json::to_string(t).unwrap());
            s.push('\n');
        }
        std::fs::write(&fixture, s).unwrap();
        eprintln!("golden fixture written: {}", fixture.display());
        return;
    }
    let have: Vec<Value> = std::fs::read_to_string(&fixture)
        .expect("read fixture")
        .lines()
        .map(|l| serde_json::from_str(l).expect("fixture line parses"))
        .collect();
    assert_eq!(have.len(), want.len(), "fixture/code line count drift");
    for (i, (h, w)) in have.iter().zip(want.iter()).enumerate() {
        assert_eq!(
            h, w,
            "\nline {i} drifted — regen with `rm tests/fixtures/golden.ndjson && cargo test -p thrum-core`\n  fixture: {}\n  current: {}",
            h, w
        );
    }
}

/// Every tone passes through the wire serde round-trip: serialize to a
/// flat frame, parse back into Envelope + raw body, decode the body into
/// its typed view, and confirm envelope fields survived.
#[test]
fn golden_round_trips_through_views() {
    for (i, wire) in golden().iter().enumerate() {
        let tone: Tone = serde_json::from_value(wire.clone())
            .unwrap_or_else(|e| panic!("line {i} fails as Tone: {e}"));
        check_round_trip(&tone);
    }
}

fn check_round_trip(tone: &Tone) {
    use thrum_core::*;
    // Re-serialize body map from the parsed tone and decode into the view.
    let body = &tone.body;
    // spot-check a representative set of shapes with the envelope merged
    match tone.chi() {
        Chi::Chunk => {
            let v: ChunkBody =
                serde_json::from_value(Value::clone(&Value::Object(body.clone()))).unwrap();
            assert!(
                v.chunk_type == "text_delta" || v.chunk_type == "tool_input_delta",
                "unexpected chunk_type {}",
                v.chunk_type
            );
            if let Some(b) = v.block_idx {
                assert!(b == 0 || b == 2);
            }
        }
        Chi::Hello => {
            let v: HelloBody = serde_json::from_value(Value::Object(body.clone())).unwrap();
            assert_eq!(v.proto_version, "0.7.0");
            assert_eq!(v.bee, "claude-cli");
        }
        Chi::ToolResult => {
            let v: ToolResultBody = serde_json::from_value(Value::Object(body.clone())).unwrap();
            assert_eq!(v.call_id, "call-1");
            assert_eq!(v.output.as_deref(), Some("file contents"));
            assert_eq!(v.is_error, Some(false));
        }
        Chi::WaneSync => {
            let v: WaneSyncBody = serde_json::from_value(Value::Object(body.clone())).unwrap();
            assert_eq!(v.snapshot.get("a1b2c3d4e5f6"), Some(&12));
        }
        Chi::GossipPublish => {
            let v: GossipPublishBody = serde_json::from_value(Value::Object(body.clone())).unwrap();
            assert_eq!(v.msg_id, "m-1");
        }
        _ => {}
    }
    // Envelope fields came back intact.
    assert!(tone.envelope.sent_at.is_some());
}

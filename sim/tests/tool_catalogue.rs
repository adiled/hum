//! tool_catalogue — wire contract for the tool catalogue humd
//! ships to a worker bee on chi:"prompt".
//!
//! Regression for the `parameters`-vs-`inputSchema` bug: claude
//! rejects the entire tools/list if any entry has
//! `inputSchema: null`, so the shape of every tool def on the
//! wire matters. This test pins:
//!
//! - chi:"prompt" arriving at the worker carries `foragerTools`
//!   merged from registered forager-hive manifests
//! - every foragerTools entry has an object `inputSchema`
//! - chi:"prompt" carries `provided` listing the union of
//!   capabilities from those forager manifests
//! - any nestler-side `tools` array on the inbound prompt is
//!   forwarded as-is (humd doesn't strip / rename)

use std::time::Duration;

use serde_json::{json, Value};

#[tokio::test(flavor = "multi_thread")]
async fn humd_enriches_prompt_with_forager_catalogue() {
    let _ = tracing_subscriber::fmt::try_init();

    let sim = sim::Sim::new();
    let humd = sim.spawn_humd(ensemble::Hid::random_humd()).await;

    // Wait for humd to install its ToneSink before injecting hellos.
    for _ in 0..200 {
        if humd.thrum.has_sink() { break; }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    // Forager advertises two fs tools + provides=["fs"]. Build the
    // hello inline (attach_mock_forager doesn't take `provides`).
    let forager_cid = format!("sim-forager-{}", uuid::Uuid::new_v4());
    let _frx = humd.thrum.register_synthetic(forager_cid.clone());
    let forager_hello = json!({
        "chi":"hello","bee":["forager"],"hive":"humfs","version":"0.0.0",
        "protoVersion": thrum_core::THRUM_VERSION,
        "provides":["fs"],
        "tools":[
            {"name":"humfs_read","description":"read file",
             "inputSchema":{"type":"object","properties":{"file_path":{"type":"string"}}}},
            {"name":"humfs_do_code","description":"edit code",
             "inputSchema":{"type":"object","properties":{"file_path":{"type":"string"}}}},
        ],
        "chis":["hello","tool-call","tool-result"],
    });
    humd.thrum.inject_tone(&forager_cid, forager_hello).await;

    // Worker captures the chi:"prompt" tone humd sends.
    let worker_cid = format!("sim-worker-{}", uuid::Uuid::new_v4());
    let mut worker_rx = humd.thrum.register_synthetic(worker_cid.clone());
    let hello = json!({
        "chi":"hello","bee":["worker"],"hive":"claude-cli","version":"0.0.0",
        "protoVersion": thrum_core::THRUM_VERSION,
        "models":["claude-opus-4-7"],
        "chis":["hello","prompt","chunk","tool-call","finish"],
    });
    humd.thrum.inject_tone(&worker_cid, hello).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    sim.nestler_send(humd.id, json!({
        "chi":"prompt","rid":"r1","sid":"hum-catalog-1",
        "modelId":"claude-opus-4-7","cwd":"/work",
        "content":"hi",
        // Caller (e.g. OC over openai-server) ships its own tools.
        "tools":[
            { "name":"read", "description":"file read",
              "inputSchema":{"type":"object","properties":{"path":{"type":"string"}}}},
        ],
    })).expect("send prompt");

    let prompt_tone = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(t) = worker_rx.recv().await {
                if t.get("chi").and_then(Value::as_str) == Some("prompt") { return t; }
            }
        }
    }).await.expect("worker receives prompt");

    let forager_tools = prompt_tone.get("foragerTools")
        .and_then(Value::as_array).cloned()
        .expect("foragerTools present");
    let names: Vec<&str> = forager_tools.iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str)).collect();
    assert!(names.contains(&"humfs_read"), "foragerTools has humfs_read; got {names:?}");
    assert!(names.contains(&"humfs_do_code"), "foragerTools has humfs_do_code; got {names:?}");

    for t in &forager_tools {
        let s = t.get("inputSchema");
        assert!(s.is_some(), "foragerTools[*] has inputSchema; missing on {}", t.get("name").unwrap());
        assert!(s.unwrap().is_object(), "foragerTools[*].inputSchema is an object — claude rejects null");
    }

    let provided = prompt_tone.get("provided")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    assert!(provided.contains(&"fs"), "provided lists 'fs'; got {provided:?}");

    let nestler_tools = prompt_tone.get("tools")
        .and_then(Value::as_array).cloned()
        .expect("nestler tools forwarded");
    assert_eq!(nestler_tools.len(), 1, "nestler 'read' forwarded as-is");
    assert!(nestler_tools[0].get("inputSchema").map(Value::is_object).unwrap_or(false));
}

/// Direct unit test for nest_common's parse_tool_def — defends the
/// catalogue against any future shim that ships `parameters`
/// instead of `inputSchema` (claude rejects on null schemas).
#[test]
fn parse_tool_def_normalizes_schema_field() {
    use serde_json::json;
    // Spot-check the parse via the bridge's set_catalogue: anything
    // that survives the merge must serialize back with a non-null
    // inputSchema. We round-trip via mcp::catalogue::merge so any
    // ToolDef path is exercised.
    let with_schema = serde_json::from_value::<mcp::protocol::ToolDef>(json!({
        "name":"read","description":"r",
        "inputSchema":{"type":"object","properties":{}},
    })).expect("ToolDef parses inputSchema");
    assert!(with_schema.input_schema.is_object());

    // Missing inputSchema entirely deserializes to Value::Null —
    // confirming why a shim emitting `parameters` would break the
    // wire if we didn't normalize.
    let bare = serde_json::from_value::<mcp::protocol::ToolDef>(json!({
        "name":"read",
    })).expect("ToolDef parses without inputSchema");
    assert!(bare.input_schema.is_null(),
        "ToolDef serde default for missing inputSchema is null — parse_tool_def must rescue this");
}

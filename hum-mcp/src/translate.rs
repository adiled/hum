//! Shape mapping between MCP JSON-RPC frames and thrum tones.
//!
//! Each worker bee that exposes an MCP server composes these
//! helpers in its own way. mcp/ never touches the network — the
//! worker owns the listener, the auth, the session model.
//!
//! Two directions:
//!
//! - **`mcp_call_to_tone`** — `tools/call` JSON-RPC request →
//!   `chi:"tool-call"` tone the worker can ship over thrum
//! - **`tone_to_mcp_result`** — `chi:"tool-result"` tone the worker
//!   received → MCP `tools/call` `result` body

use serde_json::{json, Value};

use crate::protocol::{ToolResult, wrap_tool_result};

/// Build a `chi:"tool-call"` tone from an MCP `tools/call` request
/// payload. The caller mints the `call_id` (and is responsible for
/// correlating with the eventual tool-result). `sid` is the session
/// id the worker has been told to use for this conversation.
///
/// `params` is the JSON-RPC `params` object — the function pulls
/// `name` + `arguments` from it.
pub fn mcp_call_to_tone(sid: &str, call_id: &str, params: &Value) -> Value {
    let tool_name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    json!({
        "chi": "tool-call",
        "sid": sid,
        "callId": call_id,
        "toolName": tool_name,
        "name": tool_name,
        "args": args,
    })
}

/// Extract a `ToolResult` from a `chi:"tool-result"` tone. The
/// worker uses this on the receive side of a dispatch round-trip
/// before wrapping for MCP output.
pub fn tone_to_tool_result(tone: &Value) -> ToolResult {
    let output = tone.get("output")
        .or_else(|| tone.get("result"))
        .or_else(|| tone.get("content"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_default();
    let is_error = tone.get("isError").and_then(Value::as_bool).unwrap_or(false);
    let title = tone.get("title").and_then(Value::as_str).map(str::to_string);
    let metadata = tone.get("metadata").cloned();
    ToolResult { output, title, metadata, is_error }
}

/// One-shot helper: `chi:"tool-result"` tone → MCP `tools/call`
/// `result` JSON ready to embed in a `JsonRpcResponse::ok`.
pub fn tone_to_mcp_result(tone: &Value) -> Value {
    wrap_tool_result(tone_to_tool_result(tone))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_to_tone_carries_name_args_sid_callid() {
        let params = json!({
            "name": "humfs_read",
            "arguments": { "file_path": "/x" }
        });
        let tone = mcp_call_to_tone("hum-eggs", "call-1", &params);
        assert_eq!(tone["chi"], "tool-call");
        assert_eq!(tone["sid"], "hum-eggs");
        assert_eq!(tone["callId"], "call-1");
        assert_eq!(tone["toolName"], "humfs_read");
        assert_eq!(tone["name"], "humfs_read");
        assert_eq!(tone["args"]["file_path"], "/x");
    }

    #[test]
    fn tone_to_result_extracts_output() {
        let tone = json!({
            "chi": "tool-result",
            "sid": "x",
            "callId": "c-1",
            "output": "file contents",
        });
        let r = tone_to_tool_result(&tone);
        assert_eq!(r.output, "file contents");
        assert!(!r.is_error);
    }

    #[test]
    fn tone_to_result_accepts_alt_field_names() {
        let tone = json!({ "chi": "tool-result", "result": "via-result" });
        assert_eq!(tone_to_tool_result(&tone).output, "via-result");
        let tone = json!({ "chi": "tool-result", "content": "via-content" });
        assert_eq!(tone_to_tool_result(&tone).output, "via-content");
    }

    #[test]
    fn tone_to_mcp_result_wraps_content() {
        let tone = json!({ "chi": "tool-result", "output": "hi" });
        let mcp = tone_to_mcp_result(&tone);
        assert_eq!(mcp["content"][0]["type"], "text");
        assert_eq!(mcp["content"][0]["text"], "hi");
    }

    #[test]
    fn tone_to_mcp_result_flags_error() {
        let tone = json!({ "chi": "tool-result", "output": "boom", "isError": true });
        let mcp = tone_to_mcp_result(&tone);
        assert_eq!(mcp["isError"], true);
    }
}

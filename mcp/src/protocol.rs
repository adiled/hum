//! MCP JSON-RPC types. Mirrors the subset used by mcp/tools.ts.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcRequest {
    #[serde(default)]
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcResponse {
    pub fn ok(id: Option<Value>, result: Value) -> Self {
        Self { jsonrpc: "2.0", id, result: Some(result), error: None }
    }
    pub fn err(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError { code, message: message.into(), data: None }),
        }
    }
}

/// Advertised tool. Matches the TOOLS shape in mcp/tools.ts: `name` +
/// `description` + JSON Schema `inputSchema`. Schemas are opaque to the
/// server — Claude CLI is the only consumer.
#[derive(Debug, Clone, Serialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// Native tool execution output. The wire layer wraps `output` in MCP's
/// `content: [{type:"text", text}]` envelope. `title` and `metadata`
/// travel out-of-band over thrum in TS — kept here for parity but not
/// embedded in the JSON-RPC reply.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ToolResult {
    pub output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    #[serde(default)]
    pub is_error: bool,
}

impl ToolResult {
    pub fn text(s: impl Into<String>) -> Self {
        Self { output: s.into(), ..Default::default() }
    }
    pub fn error(s: impl Into<String>) -> Self {
        Self { output: s.into(), is_error: true, ..Default::default() }
    }
}

/// Wrap a [`ToolResult`] into MCP `tools/call` `result` shape.
pub fn wrap_tool_result(r: ToolResult) -> Value {
    let mut obj = serde_json::json!({
        "content": [{ "type": "text", "text": if r.output.is_empty() { "(no output)".to_string() } else { r.output } }],
    });
    if r.is_error {
        obj.as_object_mut().unwrap().insert("isError".into(), Value::Bool(true));
    }
    obj
}

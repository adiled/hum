//! The MCP a nurse's cells reach their tools through.
//!
//! A cell that is an agent CLI takes its tools only over MCP, so the nest
//! serves it one: JSON-RPC over HTTP on 127.0.0.1, one path per sigil,
//! `/s/<sid>`. `tools/list` answers with the tools of that sigil's latest
//! prompt, which humd filled with everything its bloom may reach.
//! `tools/call` becomes a `chi:"tendril-reach"` from the nurse, and the
//! `chi:"tendril-result"` that answers it, matched by `callId`, becomes
//! the call's result, while a `chi:"tendril-info"` tells the forager the
//! whole of it. MCP stops at the nest: past it, a tool call is a tendril.
//!
//! A tool with no name is dropped, and one with no usable schema is given
//! an empty object, because a cell's MCP client rejects the whole list
//! over a single `inputSchema: null`. `parameters`, the older spelling of
//! `inputSchema`, is read too.
//!
//! A call waits as long as the longest `bash` may run, and a little more,
//! before it gives up on its result.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use lru::LruCache;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tracing::{trace, warn};

pub const PROTOCOL_VERSION: &str = "2024-11-05";

pub const SERVER_NAME: &str = "hum";

const REACH_WAIT: Duration = Duration::from_millis(thrum_core::BASH_TIMEOUT_MOST_MS + 30_000);

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ToolDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "inputSchema", default)]
    pub input_schema: Value,
}

impl ToolDef {
    pub fn parse(v: &Value) -> Option<ToolDef> {
        let name = v.get("name").and_then(Value::as_str).filter(|n| !n.is_empty())?.to_string();
        let description = v.get("description").and_then(Value::as_str).unwrap_or("").to_string();
        let input_schema = match v.get("inputSchema").or_else(|| v.get("parameters")) {
            Some(s) if s.is_object() => s.clone(),
            _ => json!({ "type": "object", "properties": {} }),
        };
        Some(ToolDef { name, description, input_schema })
    }

    pub fn all_in(tools: Option<&Value>) -> Vec<ToolDef> {
        tools
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(ToolDef::parse).collect())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct ToolResult {
    pub output: String,
    pub is_error: bool,
}

impl ToolResult {
    pub fn of(tone: &Value) -> ToolResult {
        let output = tone
            .get("output")
            .or_else(|| tone.get("result"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let is_error = tone.get("isError").and_then(Value::as_bool).unwrap_or(false);
        ToolResult { output, is_error }
    }

    fn to_mcp(&self) -> Value {
        let text = if self.output.is_empty() { "(no output)" } else { self.output.as_str() };
        let mut result = json!({ "content": [{ "type": "text", "text": text }] });
        if self.is_error
            && let Some(obj) = result.as_object_mut()
        {
            obj.insert("isError".into(), Value::Bool(true));
        }
        result
    }
}

#[derive(Debug, Clone, Deserialize)]
struct Request {
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
struct Response {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}

impl Response {
    fn ok(id: Option<Value>, result: Value) -> Self {
        Self { jsonrpc: "2.0", id, result: Some(result), error: None }
    }

    fn err(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        let message: String = message.into();
        Self { jsonrpc: "2.0", id, result: None, error: Some(json!({ "code": code, "message": message })) }
    }
}

pub struct Bridge {
    offered: Mutex<LruCache<String, Vec<ToolDef>>>,
    pending: Mutex<HashMap<String, oneshot::Sender<Value>>>,
    reach: Arc<dyn Fn(Value) + Send + Sync>,
}

impl Bridge {
    pub fn new(sigils: NonZeroUsize, reach: Arc<dyn Fn(Value) + Send + Sync>) -> Arc<Self> {
        Arc::new(Self {
            offered: Mutex::new(LruCache::new(sigils)),
            pending: Mutex::new(HashMap::new()),
            reach,
        })
    }

    pub fn offer(&self, sid: &str, tools: Vec<ToolDef>) {
        self.offered.lock().put(sid.to_string(), tools);
    }

    pub fn forget(&self, sid: &str) {
        self.offered.lock().pop(sid);
    }

    pub fn tools(&self, sid: &str) -> Vec<ToolDef> {
        self.offered.lock().get(sid).cloned().unwrap_or_default()
    }

    pub fn resolve(&self, call_id: &str, tone: Value) -> bool {
        match self.pending.lock().remove(call_id) {
            Some(tx) => {
                let _ = tx.send(tone);
                true
            }
            None => false,
        }
    }
}

pub async fn serve(bridge: Arc<Bridge>) -> Result<SocketAddr> {
    let router = Router::new().route("/s/:sid", post(handle)).with_state(bridge);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            warn!(err = %e, "nurse.mcp.exit");
        }
    });
    Ok(addr)
}

async fn handle(
    State(bridge): State<Arc<Bridge>>,
    Path(sid): Path<String>,
    Json(req): Json<Request>,
) -> (StatusCode, Json<Option<Response>>) {
    let id = req.id.clone();
    let answer = match req.method.as_str() {
        "initialize" => Some(Response::ok(id, json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": SERVER_NAME, "version": thrum_core::THRUM_VERSION },
        }))),
        "notifications/initialized" => None,
        "tools/list" => Some(Response::ok(id, json!({ "tools": bridge.tools(&sid) }))),
        "tools/call" => Some(call(&bridge, &sid, id, req.params.unwrap_or(Value::Null)).await),
        other => Some(Response::err(id, -32601, format!("unknown method '{other}'"))),
    };
    (StatusCode::OK, Json(answer))
}

async fn call(bridge: &Bridge, sid: &str, id: Option<Value>, params: Value) -> Response {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("").to_string();
    if name.is_empty() {
        return Response::err(id, -32602, "Missing tool name");
    }
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
    let call_id = thrum_core::rid();
    let (tx, rx) = oneshot::channel::<Value>();
    bridge.pending.lock().insert(call_id.clone(), tx);
    (bridge.reach)(json!({
        "chi": "tendril-reach",
        "sid": sid,
        "callId": call_id,
        "name": name,
        "args": args,
    }));
    trace!(%sid, %name, %call_id, "nurse.mcp.tendril-reach");
    match tokio::time::timeout(REACH_WAIT, rx).await {
        Ok(Ok(tone)) => {
            let grasped = ToolResult::of(&tone);
            (bridge.reach)(json!({
                "chi": "tendril-info",
                "sid": sid,
                "callId": call_id,
                "name": name,
                "args": args,
                "result": grasped.output,
                "isError": grasped.is_error,
            }));
            Response::ok(id, grasped.to_mcp())
        }
        _ => {
            bridge.pending.lock().remove(&call_id);
            Response::err(id, -32000, format!("tendril-result for callId {call_id} never came"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream;

    fn def(name: &str) -> ToolDef {
        ToolDef { name: name.into(), description: String::new(), input_schema: json!({}) }
    }

    fn bridge(shipped: Arc<Mutex<Vec<Value>>>) -> Arc<Bridge> {
        Bridge::new(NonZeroUsize::new(4).unwrap(), Arc::new(move |t| shipped.lock().push(t)))
    }

    async fn post(addr: SocketAddr, path: &str, body: Value) -> Value {
        let path = path.to_string();
        tokio::task::spawn_blocking(move || {
            let body = body.to_string();
            let mut stream = TcpStream::connect(addr).unwrap();
            let req = format!(
                "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(req.as_bytes()).unwrap();
            let mut buf = String::new();
            stream.read_to_string(&mut buf).unwrap();
            let start = buf.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
            serde_json::from_str(&buf[start..]).unwrap()
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn each_sigil_lists_only_its_own_tools() {
        let shipped = Arc::new(Mutex::new(Vec::new()));
        let bridge = bridge(shipped);
        bridge.offer("a", vec![def("bash")]);
        bridge.offer("b", vec![def("quote")]);
        let addr = serve(bridge.clone()).await.unwrap();
        let list = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});
        let a = post(addr, "/s/a", list.clone()).await;
        assert_eq!(a["result"]["tools"][0]["name"], "bash");
        assert_eq!(a["result"]["tools"].as_array().unwrap().len(), 1);
        let b = post(addr, "/s/b", list.clone()).await;
        assert_eq!(b["result"]["tools"][0]["name"], "quote");
        bridge.forget("a");
        let gone = post(addr, "/s/a", list).await;
        assert!(gone["result"]["tools"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_call_reaches_and_its_result_comes_back() {
        let shipped = Arc::new(Mutex::new(Vec::new()));
        let bridge = bridge(shipped.clone());
        bridge.offer("s", vec![def("bash")]);
        let addr = serve(bridge.clone()).await.unwrap();
        let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "bash", "arguments": {"command": "ls"}}});
        let answer = tokio::spawn(async move { post(addr, "/s/s", call).await });
        let reach = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(t) = shipped.lock().first().cloned() {
                    return t;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(reach["chi"], "tendril-reach");
        assert_eq!(reach["sid"], "s");
        assert_eq!(reach["args"]["command"], "ls");
        let call_id = reach["callId"].as_str().unwrap().to_string();
        assert!(bridge.resolve(&call_id, json!({"chi": "tendril-result", "callId": call_id,
            "output": "boom", "isError": true})));
        let got = answer.await.unwrap();
        assert_eq!(got["result"]["content"][0]["text"], "boom");
        assert_eq!(got["result"]["isError"], true);
        let info = shipped.lock().last().cloned().unwrap();
        assert_eq!(info["chi"], "tendril-info");
        assert_eq!(info["isError"], true);
    }

    #[test]
    fn a_tool_is_read_as_its_cell_can_take_it() {
        let tools = ToolDef::all_in(Some(&json!([
            {"name": "a", "inputSchema": {"type": "object"}},
            {"name": "b", "parameters": {"type": "object", "properties": {"x": {}}}},
            {"name": "c", "inputSchema": null},
            {"description": "no name"},
        ])));
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[1].input_schema["properties"]["x"], json!({}));
        assert_eq!(tools[2].input_schema["type"], "object");
    }

    #[test]
    fn a_result_is_its_output() {
        assert_eq!(ToolResult::of(&json!({"output": "hi"})).output, "hi");
        assert_eq!(ToolResult::of(&json!({"result": "via result"})).output, "via result");
        let empty = ToolResult::of(&json!({}));
        assert_eq!(empty.to_mcp()["content"][0]["text"], "(no output)");
    }
}

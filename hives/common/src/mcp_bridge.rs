//! Worker-side MCP HTTP server.
//!
//! Each worker bee that needs to expose tools to its compute via
//! MCP spawns one of these. The bridge serves JSON-RPC at
//! `/s/<session_id>`, mapping `tools/list` to the worker's current
//! catalogue and `tools/call` to a thrum `chi:"tool-call"` tone
//! through humd. Tool results return as `chi:"tool-result"` tones
//! the worker pumps into the bridge by callId.
//!
//! The mcp/ crate is a pure library — this is where it gets used.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use parking_lot::{Mutex, RwLock};
use serde_json::Value;
use tokio::sync::oneshot;
use tracing::{trace, warn};

use mcp::protocol::{JsonRpcRequest, JsonRpcResponse, ToolDef};
use mcp::catalogue;
use mcp::translate;

/// Shared state between the MCP HTTP handlers and the worker's
/// thrum loop. The worker updates the catalogue on each
/// chi:"prompt" arrival and resolves pending tool-calls when
/// chi:"tool-result" lands.
pub struct McpBridge {
    catalogue: RwLock<CatalogueSlot>,
    pending: Mutex<HashMap<String, oneshot::Sender<Value>>>,
    /// Callback the bridge invokes to ship a `chi:"tool-call"` tone
    /// out via the worker's thrum write half. Keeps the bridge
    /// transport-agnostic — caller decides how the tone reaches
    /// humd.
    ship_tool_call: Arc<dyn Fn(Value) + Send + Sync>,
}

/// Catalogue for a session — set by the worker on each chi:"prompt"
/// arrival from the asker forager.
#[derive(Debug, Clone, Default)]
struct CatalogueSlot {
    sid: String,
    tools: Vec<ToolDef>,
}

impl McpBridge {
    pub fn new(ship_tool_call: Arc<dyn Fn(Value) + Send + Sync>) -> Arc<Self> {
        Arc::new(Self {
            catalogue: RwLock::new(CatalogueSlot::default()),
            pending: Mutex::new(HashMap::new()),
            ship_tool_call,
        })
    }

    /// Set the catalogue for an incoming session. The worker calls
    /// this when it receives a chi:"prompt" — both the forager
    /// catalogue (humd-merged) and the asker's nestler tools are
    /// composed here. `provided` is the capability list (so the
    /// merge can filter capability-overlapping nestler tools).
    pub fn set_catalogue(
        &self,
        sid: impl Into<String>,
        forager_tools: Vec<ToolDef>,
        nestler_tools: Vec<ToolDef>,
        provided: &[String],
    ) {
        let merged = catalogue::merge(forager_tools, nestler_tools, provided);
        *self.catalogue.write() = CatalogueSlot { sid: sid.into(), tools: merged };
    }

    /// Resolve a pending `tools/call` with the result from a
    /// `chi:"tool-result"` tone the worker received over thrum.
    /// Returns `true` if the callId matched a waiting handler.
    pub fn resolve(&self, call_id: &str, tone: Value) -> bool {
        if let Some(tx) = self.pending.lock().remove(call_id) {
            let _ = tx.send(tone);
            true
        } else {
            false
        }
    }
}

/// Spawn the MCP HTTP listener on an ephemeral local port. Returns
/// the bound socket address so the worker can pass it to its
/// compute (e.g. `claude --mcp-config <...url>`).
///
/// The server runs until the process exits. Spawning blocks only
/// long enough to bind; the listener task is detached.
pub async fn spawn_local_mcp(bridge: Arc<McpBridge>) -> Result<SocketAddr> {
    let router = Router::new()
        .route("/s/:sid", post(handle))
        .with_state(bridge);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            warn!(err = %e, "mcp.bridge.exit");
        }
    });
    Ok(addr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex as PlMutex;
    use serde_json::json;

    fn def(name: &str) -> ToolDef {
        ToolDef { name: name.into(), description: String::new(), input_schema: json!({}) }
    }

    #[tokio::test]
    async fn list_tools_returns_set_catalogue() {
        let shipped = Arc::new(PlMutex::new(Vec::<Value>::new()));
        let shipped_for_closure = shipped.clone();
        let bridge = McpBridge::new(Arc::new(move |t| shipped_for_closure.lock().push(t)));
        bridge.set_catalogue("hum-test", vec![def("humfs_read")], vec![], &["fs".into()]);
        let addr = spawn_local_mcp(bridge).await.expect("bind");
        let client = reqwest_get_post();
        let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}});
        let resp = client.post(format!("http://{}/s/hum-test", addr))
            .json(&body).send().await.expect("post");
        let v: Value = resp.json().await.expect("json");
        let names: Vec<&str> = v["result"]["tools"].as_array().unwrap()
            .iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"humfs_read"));
    }

    #[tokio::test]
    async fn call_tool_round_trips_via_bridge_resolve() {
        let shipped = Arc::new(PlMutex::new(Vec::<Value>::new()));
        let shipped_for_closure = shipped.clone();
        let bridge = McpBridge::new(Arc::new(move |t| shipped_for_closure.lock().push(t)));
        bridge.set_catalogue("hum-test", vec![def("humfs_read")], vec![], &["fs".into()]);
        let addr = spawn_local_mcp(bridge.clone()).await.expect("bind");
        let client = reqwest_get_post();
        // Fire-and-park: post tools/call in a task; after a moment,
        // resolve via the bridge with a fake tool-result tone.
        let url = format!("http://{}/s/hum-test", addr);
        let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":"humfs_read","arguments":{"file_path":"/x"}}});
        let call_task = tokio::spawn(async move {
            client.post(url).json(&body).send().await.unwrap().json().await.unwrap()
        });
        // Wait for the bridge to ship the tool-call so we know its callId.
        let call_id = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(tone) = shipped.lock().first().cloned() {
                    return tone["callId"].as_str().unwrap().to_string();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("tool-call shipped");
        let fake_result = json!({
            "chi":"tool-result","sid":"hum-test","callId":call_id,
            "output":"file body"
        });
        assert!(bridge.resolve(&call_id, fake_result), "callId resolved");
        let resp = call_task.await.expect("call task joined");
        assert_eq!(resp["result"]["content"][0]["text"], "file body");
    }

    // Minimal reqwest client built from std + hyper would bloat the
    // crate; just use a tiny inline TcpStream-based POST helper.
    fn reqwest_get_post() -> reqwest_lite::Client { reqwest_lite::Client::new() }

    mod reqwest_lite {
        use serde::Serialize;
        use serde_json::Value;
        use std::io::{Read, Write};
        use std::net::TcpStream;

        pub struct Client;
        impl Client {
            pub fn new() -> Self { Self }
            pub fn post(self, url: String) -> RequestBuilder {
                RequestBuilder { url, body: None }
            }
        }
        pub struct RequestBuilder {
            url: String,
            body: Option<String>,
        }
        impl RequestBuilder {
            pub fn json<T: Serialize>(mut self, v: &T) -> Self {
                self.body = Some(serde_json::to_string(v).unwrap());
                self
            }
            pub async fn send(self) -> Result<Response, std::io::Error> {
                let url = self.url;
                let body = self.body.unwrap_or_default();
                tokio::task::spawn_blocking(move || -> Result<Response, std::io::Error> {
                    let stripped = url.strip_prefix("http://").unwrap();
                    let (host, path) = stripped.split_once('/').unwrap();
                    let path = format!("/{path}");
                    let mut stream = TcpStream::connect(host)?;
                    let req = format!(
                        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream.write_all(req.as_bytes())?;
                    let mut buf = Vec::new();
                    stream.read_to_end(&mut buf)?;
                    let s = String::from_utf8_lossy(&buf).to_string();
                    let body_start = s.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
                    Ok(Response { body: s[body_start..].to_string() })
                }).await.unwrap()
            }
        }
        pub struct Response { body: String }
        impl Response {
            pub async fn json(self) -> Result<Value, serde_json::Error> {
                serde_json::from_str(&self.body)
            }
        }
    }
}

async fn handle(
    State(bridge): State<Arc<McpBridge>>,
    Path(sid): Path<String>,
    Json(req): Json<JsonRpcRequest>,
) -> (StatusCode, Json<Option<JsonRpcResponse>>) {
    let id = req.id.clone();
    match req.method.as_str() {
        "initialize" => (StatusCode::OK, Json(Some(JsonRpcResponse::ok(id, serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "hum-worker-mcp", "version": "0.30.0" },
        }))))),

        "notifications/initialized" => (StatusCode::OK, Json(None)),

        "tools/list" => {
            let slot = bridge.catalogue.read().clone();
            let tools_value = serde_json::to_value(&slot.tools).unwrap_or(serde_json::json!([]));
            (StatusCode::OK, Json(Some(JsonRpcResponse::ok(
                id,
                serde_json::json!({ "tools": tools_value }),
            ))))
        }

        "tools/call" => {
            let params = req.params.unwrap_or(Value::Null);
            let name = params.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            if name.is_empty() {
                return (StatusCode::OK, Json(Some(JsonRpcResponse::err(
                    id, -32602, "Missing tool name",
                ))));
            }
            let arguments = params.get("arguments").cloned().unwrap_or(serde_json::json!({}));
            let call_id = format!("call-{}", thrum_core::rid());
            let (tx, rx) = oneshot::channel::<Value>();
            bridge.pending.lock().insert(call_id.clone(), tx);
            let tone = translate::mcp_call_to_tone(&sid, &call_id, &params);
            (bridge.ship_tool_call)(tone);
            trace!(%sid, %name, %call_id, "mcp.bridge.tool-call.shipped");
            match tokio::time::timeout(Duration::from_secs(300), rx).await {
                Ok(Ok(tone)) => {
                    let body = translate::tone_to_mcp_result(&tone);
                    // Mirror the resolution to humd as a sid-tagged
                    // chi:"chunk" so bee shims (openai-server's
                    // /v1/responses, anthropic-server's server_tool_use
                    // path) can surface this tool call as
                    // provider-executed to the asker. Without this
                    // mirror, openai-server only sees text + finish
                    // and OC's openai-responses parser has no way to
                    // emit a `mcp_call` item with providerExecuted.
                    let output_text = body.get("content")
                        .and_then(Value::as_array)
                        .and_then(|a| a.first())
                        .and_then(|p| p.get("text"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let is_error = body.get("isError").and_then(Value::as_bool).unwrap_or(false);
                    let chunk = serde_json::json!({
                        "chi": "chunk",
                        "sid": sid,
                        "chunkType": "tool_executed",
                        "callId": call_id,
                        "toolName": name,
                        "arguments": arguments,
                        "output": output_text,
                        "isError": is_error,
                    });
                    (bridge.ship_tool_call)(chunk);
                    (StatusCode::OK, Json(Some(JsonRpcResponse::ok(id, body))))
                }
                _ => {
                    bridge.pending.lock().remove(&call_id);
                    (StatusCode::OK, Json(Some(JsonRpcResponse::err(
                        id, -32000, format!("tool-result for callId {call_id} timed out"),
                    ))))
                }
            }
        }

        other => (StatusCode::OK, Json(Some(JsonRpcResponse::err(
            id, -32601, format!("unknown method '{other}'"),
        )))),
    }
}

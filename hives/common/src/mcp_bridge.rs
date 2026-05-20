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

use mcp::protocol::{wrap_tool_result, JsonRpcRequest, JsonRpcResponse, ToolDef};
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
            let call_id = format!("call-{}", thrum_core::rid());
            let (tx, rx) = oneshot::channel::<Value>();
            bridge.pending.lock().insert(call_id.clone(), tx);
            let tone = translate::mcp_call_to_tone(&sid, &call_id, &params);
            (bridge.ship_tool_call)(tone);
            trace!(%sid, %name, %call_id, "mcp.bridge.tool-call.shipped");
            match tokio::time::timeout(Duration::from_secs(300), rx).await {
                Ok(Ok(tone)) => {
                    let body = translate::tone_to_mcp_result(&tone);
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

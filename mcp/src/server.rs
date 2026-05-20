//! HTTP MCP server. POST `/s/<session_id>` accepts one JSON-RPC frame
//! and replies with the response (or 204 for notifications). The
//! address must be loopback — caller's job to pin to 127.0.0.1.

use crate::protocol::{wrap_tool_result, JsonRpcRequest, JsonRpcResponse};
use crate::registry::Registry;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::net::SocketAddr;
use tracing::trace;

/// Spawn the MCP HTTP server bound to `addr`. Awaits until shutdown.
pub async fn serve(addr: SocketAddr, registry: Registry) -> anyhow::Result<()> {
    let app = router(registry);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(?addr, "mcp.listening");
    axum::serve(listener, app).await?;
    Ok(())
}

pub fn router(registry: Registry) -> Router {
    Router::new()
        .route("/s/:sid", post(handle))
        .route("/s/:sid", get(health))
        .route("/", get(root))
        .with_state(registry)
}

async fn root() -> impl IntoResponse { "hum-mcp" }
async fn health() -> impl IntoResponse { "hum-mcp" }

async fn handle(
    State(registry): State<Registry>,
    Path(sid): Path<String>,
    Json(req): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    trace!(method = %req.method, sid = %sid, "mcp.request.received");

    let resp = dispatch(&registry, &sid, req).await;
    match resp {
        Some(r) => (StatusCode::OK, Json(serde_json::to_value(r).unwrap())).into_response(),
        // notifications/* have no response — answer with 204.
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

async fn dispatch(
    registry: &Registry,
    sid: &str,
    req: JsonRpcRequest,
) -> Option<JsonRpcResponse> {
    let id = req.id.clone();
    match req.method.as_str() {
        "initialize" => Some(JsonRpcResponse::ok(id, json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "hum", "version": env!("CARGO_PKG_VERSION") },
        }))),

        "notifications/initialized" => None,

        "ping" => Some(JsonRpcResponse::ok(id, json!({}))),

        "tools/list" => {
            let tools = registry.list_tools(sid);
            Some(JsonRpcResponse::ok(id, json!({ "tools": tools })))
        }

        "tools/call" => {
            let params = req.params.unwrap_or(Value::Null);
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            if name.is_empty() {
                return Some(JsonRpcResponse::err(id, -32602, "Missing tool name"));
            }
            tracing::trace!(sid, %name, "mcp.tools.call");
            let result = registry.call_tool(sid, &name, args).await;
            Some(JsonRpcResponse::ok(id, wrap_tool_result(result)))
        }

        other => {
            if id.is_some() {
                Some(JsonRpcResponse::err(id, -32601, format!("Method not found: {other}")))
            } else {
                None
            }
        }
    }
}

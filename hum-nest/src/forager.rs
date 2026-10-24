//! `serve_forager` — wraps a tool-call dispatcher into a standalone
//! process that handshakes with humd via thrum.
//!
//! Mirrors `serve_worker` but for the forager side: instead of
//! spawning compute (Cell), the forager handles inbound
//! `chi:"tool-call"` tones, runs the named tool, and returns
//! `chi:"tool-result"`.
//!
//! Wire contract:
//!
//! - **Hello**: announce the hive it belongs to and the tools it owns.
//! - **Tool-call in**: humd routes `chi:"tool-call"` tones whose
//!   `toolName` matches one of the advertised tools. The forager
//!   passes the tool args to its [`ToolDispatcher`] and emits
//!   `chi:"tool-result"` carrying the response, keyed by the same
//!   `callId` that came in.
//! - **Cancel**: `chi:"cancel"` (with a `callId`) signals the forager
//!   to abort the in-flight tool, if it can.
//!
//! The wire mechanics (dial, hello, read NDJSON, reconnect) are the
//! shared `hum-thrum` client; this module is the forager-specific
//! chi semantics + tool dispatcher seam. Reconnect is built in —
//! humd restarts don't strand foragers.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;

use hum_mcp::protocol::{ToolDef, ToolResult};
use serde_json::{json, Value};
use tracing::{info, trace};

use hum_identity::enter_as;

#[async_trait]
pub trait ToolDispatcher: Send + Sync + 'static {
    fn tool_defs(&self) -> Vec<ToolDef>;
    async fn dispatch(&self, tone: Value) -> ToolResult;
}

#[derive(Debug, Clone, Default)]
pub struct ForagerAdvert {
    pub hive: String,
    pub version: String,
    pub source: Option<String>,
    pub provides: Vec<String>,
}

fn default_socket_path() -> PathBuf {
    hum_paths::thrum_sock_resolved()
}

pub async fn serve_forager<D: ToolDispatcher + 'static>(
    dispatcher: Arc<D>,
    advert: ForagerAdvert,
) -> Result<()> {
    let path = default_socket_path();
    hum_thrum::serve_forever(|| dial_and_serve(&path, dispatcher.clone(), &advert)).await
}

async fn dial_and_serve<D: ToolDispatcher + 'static>(
    path: &Path,
    dispatcher: Arc<D>,
    advert: &ForagerAdvert,
) -> Result<()> {
    info!(socket = %path.display(), hive = %advert.hive, "forager.connecting");
    let (reader, write_half) = hum_thrum::connect(path).await?;

    let bee_key = enter_as(&advert.hive, "main")
        .with_context(|| format!("enter the main bee of {}", advert.hive))?;

    let defs = dispatcher.tool_defs();
    let tools_value: Vec<Value> = defs.iter().map(|d| json!({
        "name": d.name,
        "description": d.description,
        "inputSchema": d.input_schema,
    })).collect();

    let mut hello = json!({
        "chi": "hello",
        "roles": ["forager"],
        "hive": &advert.hive,
        "version": &advert.version,
        "protoVersion": thrum_core::THRUM_VERSION,
        "tools": tools_value,
        "provides": &advert.provides,
        "chis": ["hello", "tool-call", "tool-result", "cancel", "breath", "echo"],
        "source": advert.source.clone().unwrap_or_default(),
    });
    bee_key.sign_hello(&mut hello);
    hum_thrum::send_json(&write_half, &hello).await?;
    info!(
        hive = %advert.hive,
        hid = %bee_key.hid.short(),
        tools = defs.len(),
        provides = ?advert.provides,
        "forager.hello.sent"
    );

    hum_thrum::read_tones(reader, |tone| {
        let chi = tone.get("chi").and_then(Value::as_str).unwrap_or("");
        match chi {
            "tool-call" => {
                let dispatcher = dispatcher.clone();
                let write_half = write_half.clone();
                tokio::spawn(async move {
                    let sid = tone.get("sid").and_then(Value::as_str).unwrap_or("").to_string();
                    let call_id = tone.get("callId").and_then(Value::as_str).unwrap_or("").to_string();
                    let result = dispatcher.dispatch(tone).await;
                    let body = json!({
                        "chi": "tool-result",
                        "sid": sid,
                        "callId": call_id,
                        "output": result.output,
                        "isError": result.is_error,
                        "title": result.title,
                        "metadata": result.metadata,
                    });
                    let _ = hum_thrum::send_json(&write_half, &body).await;
                });
            }
            "breath" | "echo" | "" => {}
            other => trace!(chi = other, "forager.unknown.chi"),
        }
    }).await
}

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
//! - **Hello**: announce as `bee:["forager"]`, advertise `tools` (the
//!   list of toolNames this forager owns) + `hive` (kind).
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

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use hum_identity::HidPrefix;
use mcp::protocol::{ToolDef, ToolResult};
use serde_json::{json, Value};
use tracing::{info, trace};

use crate::identity::load_or_mint_bee_key;

/// Forager-side tool dispatcher. The forager binary owns its own
/// state (cwd, fs.roots snapshot, permission cache); this trait is
/// the seam humd's tool-call router calls into.
#[async_trait]
pub trait ToolDispatcher: Send + Sync + 'static {
    /// What this forager advertises. Sent on hello + on `tools/list`
    /// queries from humd.
    fn tool_defs(&self) -> Vec<ToolDef>;
    /// Execute one tool. `tone` carries the full chi:"tool-call"
    /// envelope (sid, callId, toolName, args). Implementations should
    /// pull `args` and validate against the matching def's schema.
    async fn dispatch(&self, tone: Value) -> ToolResult;
}

/// What the forager advertises on hello.
#[derive(Debug, Clone)]
pub struct ForagerAdvert {
    /// Hive name — `humfs`, future `humfs-sandbox`, etc. Used as the
    /// kind tag on hello.
    pub hive: String,
    /// Crate version. Free-form semver.
    pub version: String,
    /// Optional source URL; humd carries it into the gossiped
    /// manifest verbatim.
    pub source: Option<String>,
    /// Capability categories this forager owns. humd uses these
    /// hive-level claims to deauthorize the same surface from other
    /// sources (native MCP tools, nestler-declared tools that fall
    /// in the named capability's well-known set). Today the only
    /// well-known capability is `"fs"`; future categories include
    /// `"net"`, `"shell"`, `"todo"`, etc.
    pub provides: Vec<String>,
}

impl Default for ForagerAdvert {
    fn default() -> Self {
        Self { hive: String::new(), version: String::new(), source: None, provides: Vec::new() }
    }
}

fn default_socket_path() -> PathBuf {
    hum_paths::thrum_sock_resolved()
}

/// Run the forager service loop. Blocks until shutdown; reconnects
/// on socket drop.
pub async fn serve_forager<D: ToolDispatcher + 'static>(
    dispatcher: Arc<D>,
    advert: ForagerAdvert,
) -> Result<()> {
    let path = default_socket_path();
    hum_thrum::serve_forever(|| dial_and_serve(&path, dispatcher.clone(), &advert)).await
}

async fn dial_and_serve<D: ToolDispatcher + 'static>(
    path: &PathBuf,
    dispatcher: Arc<D>,
    advert: &ForagerAdvert,
) -> Result<()> {
    info!(socket = %path.display(), hive = %advert.hive, "forager.connecting");
    let (reader, write_half) = hum_thrum::connect(path).await?;

    // Load (or mint) the persistent forager-bee identity. fbee_ hid
    // survives reconnect / restart; humd indexes by it.
    let bee_key = load_or_mint_bee_key(&advert.hive, HidPrefix::Fbee)
        .with_context(|| format!("load/mint fbee key for hive {}", advert.hive))?;

    let defs = dispatcher.tool_defs();
    let tool_names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    let tools_value: Vec<Value> = defs.iter().map(|d| json!({
        "name": d.name,
        "description": d.description,
        "inputSchema": d.input_schema,
    })).collect();

    let hello = json!({
        "chi": "hello",
        "bee": ["forager"],
        "hid": bee_key.hid.to_hex(),
        "from": bee_key.hid.to_hex(),
        "hive": &advert.hive,
        "version": &advert.version,
        "protoVersion": thrum_core::THRUM_VERSION,
        "tools": tools_value,
        "toolNames": tool_names,
        "provides": &advert.provides,
        "chis": ["hello", "tool-call", "tool-result", "cancel", "breath", "echo"],
        "source": advert.source.clone().unwrap_or_default(),
    });
    hum_thrum::send_json(&write_half, &hello).await?;
    info!(
        hive = %advert.hive,
        hid = %bee_key.hid.short(),
        tools = ?tool_names,
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
                    let tool_name = tone.get("toolName").and_then(Value::as_str).unwrap_or("").to_string();
                    let result = dispatcher.dispatch(tone).await;
                    let body = json!({
                        "chi": "tool-result",
                        "sid": sid,
                        "callId": call_id,
                        "toolName": tool_name,
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

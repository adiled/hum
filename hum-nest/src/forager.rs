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
//! The connection (hello, verdict, reconnect, the spec check) is the
//! shared `hum-thrum` client; this module is the forager's chi semantics
//! and the tool dispatcher seam. A refused bee ends `serve_forager` with
//! the refusal.

use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;

use hum_mcp::protocol::{ToolDef, ToolResult};
use serde_json::{json, Value};
use tracing::{info, trace};

use hum_thrum::{Bee, Event, Options, Refused};

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

pub async fn serve_forager<D: ToolDispatcher + 'static>(
    dispatcher: Arc<D>,
    advert: ForagerAdvert,
) -> Result<()> {
    let defs = dispatcher.tool_defs();
    let tools_value: Vec<Value> = defs.iter().map(|d| json!({
        "name": d.name,
        "description": d.description,
        "inputSchema": d.input_schema,
    })).collect();
    let hello = json!({
        "roles": ["forager"],
        "version": &advert.version,
        "tools": tools_value,
        "provides": &advert.provides,
        "chis": ["hello", "tool-call", "tool-result", "cancel", "breath", "echo"],
        "source": advert.source.clone().unwrap_or_default(),
    });
    let mut bee = Bee::link(Options::new(&advert.hive, "main").hello(hello))
        .await
        .with_context(|| format!("link the main bee of {}", advert.hive))?;
    info!(
        hive = %advert.hive,
        bee = %bee.id(),
        tools = defs.len(),
        provides = ?advert.provides,
        "forager.nestled"
    );
    let sender = bee.sender();
    while let Some(event) = bee.next().await {
        let tone = match event {
            Event::Tone(tone) => tone,
            Event::Refused(why) => return Err(Refused(why).into()),
            Event::Nestled | Event::Lost | Event::Advisory(_) => continue,
        };
        let chi = tone.get("chi").and_then(Value::as_str).unwrap_or("");
        match chi {
            "tool-call" => {
                let dispatcher = dispatcher.clone();
                let sender = sender.clone();
                tokio::spawn(async move {
                    let sid = tone.get("sid").and_then(Value::as_str).unwrap_or("").to_string();
                    let call_id = tone.get("callId").and_then(Value::as_str).unwrap_or("").to_string();
                    let result = dispatcher.dispatch(tone).await;
                    sender.send(json!({
                        "chi": "tool-result",
                        "sid": sid,
                        "callId": call_id,
                        "output": result.output,
                        "isError": result.is_error,
                        "title": result.title,
                        "metadata": result.metadata,
                    }));
                });
            }
            "breath" | "echo" | "" => {}
            other => trace!(chi = other, "forager.unknown.chi"),
        }
    }
    Ok(())
}

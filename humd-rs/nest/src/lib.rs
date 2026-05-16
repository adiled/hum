//! nest — Claude subprocess pool with two perches.
//!
//! Mirrors the TS `nest/` module: a `Nest` keyed by `pool_key` (== sid) holds
//! at most one `Roost` per key. Each roost wraps a child process spawned via
//! a `Perch` (pipe or PTY). The daemon binary registers `Listener`s on a
//! roost to receive parsed Claude stream events (`Petal`s).
//!
//! v0 scope: PipePerch is real; PtyPerch is a stub that compiles. No
//! LLM-classifier readiness, no inject-health.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::{mpsc, Mutex};

pub mod pipe;
pub mod pool;
pub mod pty;

pub use pipe::PipePerch;
pub use pool::Nest;
pub use pty::PtyPerch;

/// Args the daemon hands to a perch when it asks for a fresh roost.
#[derive(Debug, Clone)]
pub struct PerchSpawnArgs {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: HashMap<String, String>,
    /// PTY mode only: id the harness expects the transcript to land at.
    pub harness_session_id: Option<String>,
    /// PTY mode only: JSONL transcript file path the harness polls.
    pub transcript_path: Option<String>,
}

/// A Roost is one live Claude subprocess seen from the daemon side.
/// Pipe and PTY perches both produce this same shape.
pub struct Roost {
    pub pid: Option<u32>,
    /// Send raw NDJSON lines (already serialized, no trailing newline) to the
    /// child's stdin. PipePerch writes them straight through; PtyPerch
    /// translates `{type:"user",...}` into typed text + Enter.
    pub stdin: mpsc::Sender<String>,
    /// Parsed Claude stream events. Each Value is one JSON message off
    /// stdout. The daemon binary turns these into thrum petals.
    pub events: Arc<Mutex<mpsc::Receiver<Value>>>,
    /// Resolves with the child's exit code once it terminates.
    pub exited: tokio::sync::oneshot::Receiver<i32>,
    /// True for PTY/REPL-style roosts the pool evicts on each `result`.
    pub ephemeral: bool,
    /// Kill the child. Best-effort; safe to call multiple times.
    pub kill: Arc<dyn Fn() + Send + Sync>,
}

#[async_trait]
pub trait Perch: Send + Sync {
    fn ephemeral(&self) -> bool;
    async fn spawn(&self, args: PerchSpawnArgs) -> Result<Roost>;
}

/// Listener receives parsed Claude stream events for one session bound to a
/// roost. Mirrors TS `BloomListener`. The daemon binary is responsible for
/// translating Petals into thrum chunks.
#[async_trait]
pub trait Listener: Send + Sync {
    fn session_id(&self) -> &str;
    async fn on_petal(&self, kind: &str, payload: Value);
    async fn on_roost(&self, nest_id: &str, model: &str, tools: Vec<String>);
    async fn on_wilt(&self, finish_reason: &str, usage: Option<Value>, provider_meta: Value);
    async fn on_thorn(&self, wound: &str);
}

/// Encode a user prompt for stream-json stdin (TS `encodePrompt`).
pub fn encode_prompt(text: &str) -> String {
    serde_json::json!({
        "type": "user",
        "message": { "role": "user", "content": [{"type": "text", "text": text}] }
    })
    .to_string()
}

/// Encode a tool_result reply for stream-json stdin (TS `encodeToolResult`).
pub fn encode_tool_result(tool_use_id: &str, result: &str) -> String {
    serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": result,
            }]
        }
    })
    .to_string()
}

/// Encode a `control_cancel_request` (mid-turn interrupt) for pipe-mode stdin.
pub fn encode_cancel(request_id: &str) -> String {
    serde_json::json!({
        "type": "control_cancel_request",
        "request_id": request_id,
    })
    .to_string()
}

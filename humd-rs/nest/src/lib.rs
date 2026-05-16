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

pub mod pool;

pub use pool::Nest;

/// High-level spec the daemon hands to a perch. The perch is responsible
/// for turning this into whatever command line / process invocation its
/// underlying harness needs — claude CLI args, env vars, etc.  The daemon
/// stays harness-agnostic.
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    /// hum session id for this roost.
    pub sid: String,
    /// Model id to run on (e.g. "claude-sonnet-4-6", "claude-haiku-4-5").
    pub model_id: String,
    /// Working directory for the spawned process. Drives transcript
    /// location and fs MCP grounding.
    pub cwd: String,
    /// Optional system prompt to prepend to the conversation.
    pub system_prompt: Option<String>,
    /// Optional MCP HTTP base URL (e.g. "http://127.0.0.1:29147"). The
    /// perch wires this into its harness's MCP config so the spawned
    /// process can reach hum's tool surface.
    pub mcp_url: Option<String>,
    /// Optional path to the claude CLI binary. None → "claude" on PATH.
    pub cli_path: Option<String>,
    /// Optional resume id — the harness uses this to pick up an existing
    /// transcript instead of starting fresh.
    pub resume_id: Option<String>,
    /// Plan mode — disables adaptive-thinking env.
    pub plan_mode: bool,
    /// Permissions allowlist names — passed to the harness's tool filter.
    pub permissions: Vec<String>,
    /// Allowed tool names — narrows what the harness advertises.
    pub allowed_tools: Vec<String>,
    /// Extra env overrides spread onto the spawn (after defaults).
    pub env: HashMap<String, String>,
}

impl SpawnSpec {
    pub fn new(sid: impl Into<String>, model_id: impl Into<String>, cwd: impl Into<String>) -> Self {
        Self {
            sid: sid.into(),
            model_id: model_id.into(),
            cwd: cwd.into(),
            system_prompt: None,
            mcp_url: None,
            cli_path: None,
            resume_id: None,
            plan_mode: false,
            permissions: Vec::new(),
            allowed_tools: Vec::new(),
            env: HashMap::new(),
        }
    }
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
    async fn spawn(&self, spec: SpawnSpec) -> Result<Roost>;
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

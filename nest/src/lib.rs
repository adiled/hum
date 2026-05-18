//! nest — bee crates: traits for WorkerBee (produce compute) and
//! ForagerBee (translate outside wire ↔ thrum). A `Nest` keyed by
//! `pool_key` (== sid) holds at most one `Roost` per key. Each roost
//! wraps a child process spawned via a `WorkerBee` impl. The daemon
//! binary registers `Listener`s on a roost to receive parsed stream
//! events.
//!
//! These traits are the Rust SDK for building bees that handshake with
//! humd over thrum. Authors who don't want Rust can implement the same
//! wire role directly via the thrum-clients libs.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::{mpsc, Mutex};

pub mod mock;
pub mod pool;

// Resource-oriented primitives for the Roost as a system resource.
// Filled in by parallel work; declared together so contributors don't race
// on this file. See each module's docstring for scope.
pub mod metrics;   // per-roost observability (RSS, CPU, fds)
pub mod limits;    // per-roost OS-level caps (rlimit, cgroups)
pub mod budget;    // per-roost soft caps (tokens, tool-call rates)
pub mod health;    // pool-wide pressure tiers + eviction policy

pub use mock::MockWorkerBee;
pub use pool::Nest;

/// High-level spec the daemon hands to a worker bee. The bee is
/// responsible for turning this into whatever command line / process
/// invocation its underlying harness needs — CLI args, env vars, etc.
/// The daemon stays harness-agnostic.
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
    /// Tool names the harness must refuse. Populated by humd from the
    /// nestler's hello (e.g. when the nestler opts into mcp-exclusive
    /// fs control by listing built-ins here). The worker bee passes
    /// it through verbatim; it does not invent the list.
    pub disallowed_tools: Vec<String>,
    /// Extra env overrides spread onto the spawn (after defaults).
    pub env: HashMap<String, String>,
    /// OS-level caps the WorkerBee impl applies to the spawned child via
    /// `Command::pre_exec` (Linux) or no-op (other platforms).
    /// Default: empty — child inherits the parent's limits.
    pub resource_limits: limits::ResourceLimits,
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
            disallowed_tools: Vec::new(),
            env: HashMap::new(),
            resource_limits: limits::ResourceLimits::default(),
        }
    }
}

/// A Roost is one live subprocess seen from the daemon side. Pipe and
/// PTY worker bees both produce this same shape.
pub struct Roost {
    pub pid: Option<u32>,
    /// Send raw NDJSON lines (already serialized, no trailing newline) to the
    /// child's stdin. PipePerch writes them straight through; PtyPerch
    /// translates `{type:"user",...}` into typed text + Enter.
    pub stdin: mpsc::Sender<String>,
    /// Parsed stream events. Each Value is one JSON message off
    /// stdout. The daemon binary turns these into thrum petals.
    pub events: Arc<Mutex<mpsc::Receiver<Value>>>,
    /// Resolves with the child's exit code once it terminates.
    pub exited: tokio::sync::oneshot::Receiver<i32>,
    /// True for PTY/REPL-style roosts the pool evicts on each `result`.
    pub ephemeral: bool,
    /// Kill the child. Best-effort; safe to call multiple times.
    pub kill: Arc<dyn Fn() + Send + Sync>,
}

/// Statefulness propensity of a bee — the same axis hives carry
/// (see `hives/README.md`), declared from the bee side so peers can
/// adapt prompt assembly.
///
/// - `StatefulSession` — the bee process retains conversation state
///   in its own memory across multiple prompts that share a sid (e.g.
///   `claude -p` stream-json, REPL pty). Foragers should derive a
///   stable sid per OC/external conversation and send only the *new*
///   user turn each request; the worker keeps prior history.
/// - `StatelessPerCall` — the bee treats every call as a fresh
///   self-contained request (e.g. direct API workers, paid oracles).
///   Foragers should send full transcript every time; pooling buys
///   nothing.
/// - `EphemeralPerCall` — like Stateless but the bee process exits
///   after a single result. Eviction is automatic; sid stability is
///   irrelevant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Propensity {
    StatefulSession,
    StatelessPerCall,
    EphemeralPerCall,
}

/// A WorkerBee produces compute — it spawns a roost (subprocess or
/// in-process inference) when handed a `SpawnSpec`. This is the trait
/// any compute-side bee implements to be commissioned by a hive.
#[async_trait]
pub trait WorkerBee: Send + Sync {
    fn ephemeral(&self) -> bool;
    /// What kind of state machine does this worker run? Default
    /// implementation is conservative (`EphemeralPerCall`) so any new
    /// worker that forgets to override gets correct full-history
    /// behavior at the cost of perf, not the other way around.
    fn propensity(&self) -> Propensity {
        if self.ephemeral() { Propensity::EphemeralPerCall } else { Propensity::StatefulSession }
    }
    async fn spawn(&self, spec: SpawnSpec) -> Result<Roost>;
}

/// A ForagerBee translates an outside wire (OpenAI, Anthropic, custom
/// HTTP, etc.) into thrum tones and back. It carries `chi:"prompt"` in
/// and `chi:"chunk"` / `chi:"finish"` / `chi:"tool-call"` out, against
/// some external surface.
///
/// Hybrid bees that are both worker and forager simply implement both
/// traits — there is no constraint against it.
#[async_trait]
pub trait ForagerBee: Send + Sync {
    /// Symbolic name for the external surface this forager translates
    /// (e.g. "openai-v1", "anthropic-messages").
    fn surface(&self) -> &str;
}

/// Listener receives parsed stream events for one session bound to a
/// roost. The daemon binary is responsible for translating Petals into
/// thrum chunks.
#[async_trait]
pub trait Listener: Send + Sync {
    fn session_id(&self) -> &str;
    async fn on_petal(&self, kind: &str, payload: Value);
    async fn on_roost(&self, nest_id: &str, model: &str, tools: Vec<String>);
    async fn on_wilt(&self, finish_reason: &str, usage: Option<Value>, provider_meta: Value);
    async fn on_thorn(&self, wound: &str);
}

/// A non-text addition to a prompt — image, audio, pdf, etc. Carried
/// alongside the text content so workers can hand the model both at
/// once. `data` is base64 for inline; `url` is the alternative (worker
/// dereferences). Exactly one of `data` / `url` should be set.
#[derive(Debug, Clone)]
pub struct Attachment {
    /// Content category. "image" / "audio" / "pdf" / "video" / "file".
    /// Workers decide which kinds they can route to the model.
    pub kind: String,
    /// IANA media type — "image/png", "audio/wav", "application/pdf".
    pub media_type: String,
    /// Inline base64 payload.
    pub data: Option<String>,
    /// URL the worker can dereference. Either http(s) or a data: URI.
    pub url: Option<String>,
}

/// Encode a user prompt for stream-json stdin. Wraps the text in the
/// content-block shape stream-json workers expect. Use
/// `encode_prompt_with_attachments` for multimodal prompts.
pub fn encode_prompt(text: &str) -> String {
    encode_prompt_with_attachments(text, &[])
}

/// Encode a user prompt with non-text attachments alongside. Image
/// attachments translate to a base64 / url image block; other kinds
/// (audio, pdf, video) pass the media_type through so receiving
/// workers can opt-in as their model surface grows. Unknown kinds
/// without a known encoding fall back to a text annotation so the
/// model at least sees that an attachment was present.
pub fn encode_prompt_with_attachments(text: &str, attachments: &[Attachment]) -> String {
    let mut content: Vec<Value> = vec![serde_json::json!({"type": "text", "text": text})];
    for att in attachments {
        match att.kind.as_str() {
            "image" => {
                if let Some(data) = att.data.as_ref() {
                    content.push(serde_json::json!({
                        "type": "image",
                        "source": { "type": "base64", "media_type": att.media_type, "data": data }
                    }));
                } else if let Some(url) = att.url.as_ref() {
                    content.push(serde_json::json!({
                        "type": "image",
                        "source": { "type": "url", "url": url }
                    }));
                }
            }
            other => {
                // Unknown attachment kind — surface its presence in
                // text so the model can at least acknowledge it,
                // rather than silently dropping. Workers that learn
                // to handle new kinds (audio, pdf) take over the
                // proper translation later.
                let where_clause = att.url.as_deref()
                    .map(|u| format!(" ({u})"))
                    .unwrap_or_default();
                content.push(serde_json::json!({
                    "type": "text",
                    "text": format!("[attachment: kind={other} media_type={}{where_clause}]", att.media_type)
                }));
            }
        }
    }
    serde_json::json!({
        "type": "user",
        "message": { "role": "user", "content": content }
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

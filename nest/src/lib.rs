//! WorkerBee trait + Cell shape + lifecycle/limits/metrics submodules.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use ids::HumId;
use serde_json::Value;
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;

pub mod lifecycle;
pub mod metrics;
pub mod limits;

/// An egg — what a worker bee needs to raise a cell.
#[derive(Debug, Clone)]
pub struct Egg {
    /// Canonical hum session id. Foreign-format projections (claude
    /// `--session-id`) derive from this via `sid.to_uuid_v5(NS_*)`.
    pub sid: HumId,
    /// Model id to run on (e.g. "claude-sonnet-4-6", "claude-haiku-4-5").
    pub model_id: String,
    /// Working directory for the spawned process. Drives transcript
    /// location and fs MCP grounding.
    pub cwd: String,
    /// Optional system prompt to prepend to the conversation.
    pub system_prompt: Option<String>,
    /// Optional MCP HTTP base URL (e.g. "http://127.0.0.1:29147"). The
    /// worker wires this into its harness's MCP config so the spawned
    /// process can reach hum's tool surface.
    pub mcp_url: Option<String>,
    /// Optional path to the claude CLI binary. None → "claude" on PATH.
    pub cli_path: Option<String>,
    /// Foreign resume id — a claude session UUID the harness should pick
    /// up instead of the sid-derived one. Stays String because it's a
    /// foreign identifier hum did not mint.
    pub resume_id: Option<String>,
    /// True = create a new claude session under the sid-derived UUID.
    /// False = try to resume that UUID. Worker flips to true on resume
    /// miss. Ignored when `resume_id` is set.
    pub fresh: bool,
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
    pub bounds: limits::Bounds,
}

impl Egg {
    pub fn new(sid: HumId, model_id: impl Into<String>, cwd: impl Into<String>) -> Self {
        Self {
            sid,
            model_id: model_id.into(),
            cwd: cwd.into(),
            system_prompt: None,
            mcp_url: None,
            cli_path: None,
            resume_id: None,
            fresh: false,
            plan_mode: false,
            permissions: Vec::new(),
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            env: HashMap::new(),
            bounds: limits::Bounds::default(),
        }
    }
}

/// One brood cell — a living subprocess raised inside a bee.
pub struct Cell {
    pub mark: Option<u32>,
    pub feed: mpsc::Sender<String>,
    pub mmm: Arc<Mutex<mpsc::Receiver<Value>>>,
    pub emerged: tokio::sync::oneshot::Receiver<i32>,
    pub ephemeral: bool,
    pub silence: CancellationToken,
}

impl Cell {
    /// Still the cell — SIGKILL + reap, idempotent.
    pub fn still(&self) { self.silence.cancel(); }
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

/// What a curation trimmed. Byte counts are of the bee's own stored
/// transcript, whatever form that takes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CurateReport {
    pub bytes_before: u64,
    pub bytes_after: u64,
}

impl CurateReport {
    pub fn trimmed(&self) -> u64 {
        self.bytes_before.saturating_sub(self.bytes_after)
    }
}

/// A WorkerBee raises cells from eggs — the compute-side trait every
/// commissioned hive implements.
#[async_trait]
pub trait WorkerBee: Send + Sync {
    fn ephemeral(&self) -> bool;
    fn propensity(&self) -> Propensity {
        if self.ephemeral() { Propensity::EphemeralPerCall } else { Propensity::StatefulSession }
    }
    async fn raise(&self, egg: Egg) -> Result<Cell>;

    /// Curate the stored transcript for a sid — trim what can go without
    /// losing the thread. Answers `chi:"curate"`.
    ///
    /// Operates on what the bee has persisted, not on a live cell: a
    /// harness that exits between turns (claude `-p`) still has a
    /// transcript to curate, and a curate may well arrive with nothing
    /// running. Default is a no-op for bees holding nothing to trim.
    async fn curate(&self, _sid: &HumId, _cwd: &str) -> Result<CurateReport> {
        Ok(CurateReport::default())
    }
}

/// Pollen — what a forager bee carries back alongside the text:
/// images, audio, pdf, video, files.
#[derive(Debug, Clone)]
pub struct Pollen {
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
/// `encode_prompt_with_pollen` for multimodal prompts.
pub fn encode_prompt(text: &str) -> String {
    encode_prompt_with_pollen(text, &[])
}

/// Encode a user prompt with non-text attachments alongside. Image
/// attachments translate to a base64 / url image block; other kinds
/// (audio, pdf, video) pass the media_type through so receiving
/// workers can opt-in as their model surface grows. Unknown kinds
/// without a known encoding fall back to a text annotation so the
/// model at least sees that an attachment was present.
pub fn encode_prompt_with_pollen(text: &str, pollen: &[Pollen]) -> String {
    let mut content: Vec<Value> = vec![serde_json::json!({"type": "text", "text": text})];
    for grain in pollen {
        match grain.kind.as_str() {
            "image" => {
                if let Some(data) = grain.data.as_ref() {
                    content.push(serde_json::json!({
                        "type": "image",
                        "source": { "type": "base64", "media_type": grain.media_type, "data": data }
                    }));
                } else if let Some(url) = grain.url.as_ref() {
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
                let where_clause = grain.url.as_deref()
                    .map(|u| format!(" ({u})"))
                    .unwrap_or_default();
                content.push(serde_json::json!({
                    "type": "text",
                    "text": format!("[attachment: kind={other} media_type={}{where_clause}]", grain.media_type)
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

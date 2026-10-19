//! hum's hive-side kernel. The worker contract ([`WorkerBee`], [`Egg`],
//! [`Cell`], [`Propensity`], [`Pollen`], tone encoders), the process
//! supervision runtime ([`lifecycle`] group-owning / [`metrics`]
//! sampling / [`limits`] rlimits), and the wire-semantics loops that
//! wrap [`hum-thrum`] for the two bee shapes — [`serve`] (`serve_worker`)
//! and [`forager`] (`serve_forager`).
//!
//! One crate, no `nest`-common indirection: a hive that needs the nest
//! lives here; a remote hive that only needs the wire takes the leaf
//! crates ([`hum-thrum`], [`hum-identity`], [`hum-mcp`]) directly.
//!
//! The drone (humd's sentinel)-facing [`RegexClassifier`](suspicion_regex::RegexClassifier)
//! also lives here (patterns for chat-LLM context-loss detection).
//!
//! [`Propensity`] is the statefulness axis hives carry in
//! `hives/README.md`, declared from the bee side so peers can adapt
//! prompt assembly:
//!
//! - `StatefulSession` — the bee process retains conversation state in
//!   its own memory across multiple prompts that share a sid. Foragers
//!   should derive a stable sid per external conversation and send only
//!   the *new* user turn each request; the worker keeps prior history.
//! - `StatelessPerCall` — the bee treats every call as a fresh
//!   self-contained request. Foragers should send the full transcript
//!   every time; pooling buys nothing.
//! - `EphemeralPerCall` — like `StatelessPerCall` but the bee process
//!   exits after a single result. Eviction is automatic; sid stability
//!   is irrelevant.
//!
//! [`Suspicion`] is how loud a classifier is shouting about a piece of
//! LLM output. `None` means the text looks fine. `Soft` is flagged for
//! evaluator-driven adjudication. `Heavy` is strongly flagged, though
//! the evaluator may still confirm it. `Critical` bypasses the
//! evaluator and swallows immediately.
//!
//! [`Classifier`] is the context-loss heuristic seam. The drone calls
//! it on `TurnEnd` and during `assess` to score the accumulated
//! response text. Implementations decide which patterns are which
//! severity; the drone only branches on the returned [`Suspicion`].
//! The default is [`NoopClassifier`], which makes the drone a pure
//! channel-health sentinel that never reaches the swallow path on its
//! own.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use hum_identity::HumId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;

pub mod forager;
pub mod lifecycle;
pub mod limits;
pub mod metrics;
pub mod serve;
pub mod suspicion_regex;

pub use forager::{serve_forager, ForagerAdvert, ToolDispatcher};
pub use hum_mcp::protocol::{ToolDef, ToolResult};
pub use serve::{serve_worker, HiveAdvert};
pub use suspicion_regex::RegexClassifier;

#[derive(Debug, Clone)]
pub struct Egg {
    pub sid: HumId,
    pub model_id: String,
    pub cwd: String,
    pub system_prompt: Option<String>,
    pub mcp_url: Option<String>,
    pub cli_path: Option<String>,
    pub resume_id: Option<String>,
    pub fresh: bool,
    pub plan_mode: bool,
    pub permissions: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    pub env: HashMap<String, String>,
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

pub struct Cell {
    pub mark: Option<u32>,
    pub feed: mpsc::Sender<String>,
    pub mmm: Arc<Mutex<mpsc::Receiver<Value>>>,
    pub emerged: tokio::sync::oneshot::Receiver<i32>,
    pub ephemeral: bool,
    pub silence: CancellationToken,
}

impl Cell {
    pub fn still(&self) { self.silence.cancel(); }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Propensity {
    StatefulSession,
    StatelessPerCall,
    EphemeralPerCall,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Suspicion {
    None,
    Soft,
    Heavy,
    Critical,
}

impl Suspicion {
    pub fn flagged(self) -> bool {
        !matches!(self, Suspicion::None)
    }
}

pub trait Classifier: Send + Sync {
    fn classify(&self, text: &str) -> Suspicion;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopClassifier;

impl Classifier for NoopClassifier {
    fn classify(&self, _text: &str) -> Suspicion {
        Suspicion::None
    }
}

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
    /// harness that exits between turns still has a transcript to
    /// curate, and a curate may well arrive with nothing running.
    async fn curate(&self, _sid: &HumId, _cwd: &str) -> Result<CurateReport> {
        Ok(CurateReport::default())
    }
}

#[derive(Debug, Clone)]
pub struct Pollen {
    pub kind: String,
    pub media_type: String,
    pub data: Option<String>,
    pub url: Option<String>,
}

pub fn encode_prompt(text: &str) -> String {
    encode_prompt_with_pollen(text, &[])
}

/// Encode a user prompt with non-text attachments alongside. Image
/// attachments translate to a base64 / url image block; other kinds
/// pass the media_type through so receiving workers can opt-in as their
/// model surface grows. Unknown kinds without a known encoding fall
/// back to a text annotation.
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
                // text rather than silently dropping it.
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
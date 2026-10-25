//! hum's hive-side kernel for a nurse. The nurse contract ([`Nurse`],
//! [`Egg`], [`Cell`], [`Propensity`]), the process supervision runtime
//! ([`lifecycle`] group-owning / [`metrics`] sampling / [`limits`]
//! rlimits), the [`mcp`] its cells reach their tools through, and the
//! wire-semantics loop that wraps [`hum-thrum`] for a nurse: [`serve`]
//! (`serve_nurse`).
//!
//! A cell speaks the thrum and nothing else to the nest. It is fed its
//! sigil's tones, each prompt, an answer to a tool it reached outside its
//! sigil's tools, a cancel, and gives back the tones of what it grew. What
//! the engine behind it speaks is the hive's own to translate.
//!
//! One crate, no `nest`-common indirection: a hive that needs the nest
//! lives here; a remote hive that only needs the wire takes the leaf
//! crates ([`hum-thrum`], [`hum-identity`]) directly.
//!
//! A cell never works a host of its own. It runs in its nest's directory,
//! and whatever it does to the world it does through a tendril, `bash` to
//! a builder above all, so the kernel sees every reach.
//!
//! An [`Egg`] is what any nurse raises a cell from: the sigil, the model,
//! the nest's directory, the sigil's MCP address, the environment and
//! limits, and the prompt the cell is raised for, the blooms its forager
//! grafted on it among the rest, so a cell that did not grow them grows
//! from them. A nurse that can grow a bloom on from its graft, with
//! no new words, says so with [`Nurse::grows_on`]; any other is never
//! handed such a prompt.
//!
//! The drone (humd's sentinel)-facing [`RegexClassifier`](suspicion_regex::RegexClassifier)
//! also lives here (patterns for chat-LLM context-loss detection).
//!
//! [`Propensity`] is the statefulness axis hives carry in
//! `hives/README.md`, declared from the bee side so peers can adapt
//! prompt assembly:
//!
//! - `StatefulSession` — the nurse keeps a sigil's conversation itself
//!   across the prompts that share its sid, so a forager that keeps one
//!   sid per conversation sends only the new turn, and a prompt's graft
//!   matters only to a cell that did not grow those blooms.
//! - `StatelessPerCall` — the nurse keeps nothing between calls and
//!   grows each bloom from its prompt's graft; pooling buys nothing.
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
//! it when a bloom wilts and during `assess` to score the accumulated
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

pub mod lifecycle;
pub mod limits;
pub mod mcp;
pub mod metrics;
pub mod nest;
pub mod serve;
pub mod suspicion_regex;

pub use mcp::{ToolDef, ToolResult};
pub use nest::NestConfig;
pub use serve::{serve_nurse, NurseAdvert};
pub use suspicion_regex::RegexClassifier;

#[derive(Debug, Clone)]
pub struct Egg {
    pub sid: HumId,
    pub model_id: String,
    pub cwd: String,
    pub mcp_url: Option<String>,
    pub env: HashMap<String, String>,
    pub bounds: limits::Bounds,
    pub prompt: thrum_core::PromptBody,
}

impl Egg {
    pub fn new(sid: HumId, model_id: impl Into<String>, cwd: impl Into<String>) -> Self {
        Self {
            sid,
            model_id: model_id.into(),
            cwd: cwd.into(),
            mcp_url: None,
            env: HashMap::new(),
            bounds: limits::Bounds::default(),
            prompt: thrum_core::PromptBody::default(),
        }
    }
}

pub struct Cell {
    pub mark: Option<u32>,
    pub feed: mpsc::Sender<Value>,
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
pub trait Nurse: Send + Sync {
    fn ephemeral(&self) -> bool;
    fn propensity(&self) -> Propensity {
        if self.ephemeral() { Propensity::EphemeralPerCall } else { Propensity::StatefulSession }
    }
    async fn raise(&self, egg: Egg) -> Result<Cell>;

    fn grows_on(&self) -> bool {
        false
    }

    async fn curate(&self, _sid: &HumId, _cwd: &str) -> Result<CurateReport> {
        Ok(CurateReport::default())
    }
}

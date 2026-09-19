//! The process runtime behind hum's worker hives — owns child process
//! groups (command-group), per-cell metrics sampling (sysinfo), and the
//! cancel/tree-kill plumbing. Humd itself never imports this crate; only
//! worker harnesses that actually spawn model processes (claude-cli,
//! claude-repl) do.
//!
//! The compute *contract* — [`WorkerBee`], [`Egg`], [`Cell`],
//! [`Propensity`], [`Pollen`], the tone encoders and
//! [`limits::Bounds`] — lives in [`hum-nest`], a daemon-independent
//! leaf. `nest` re-exports it for back-compat so existing call sites
//! keep resolving during the migration.

pub mod lifecycle;
pub mod metrics;

pub use hum_nest::{
    limits, Cell, Egg, Pollen, Propensity, WorkerBee, encode_cancel, encode_prompt,
    encode_prompt_with_pollen, encode_tool_result,
};
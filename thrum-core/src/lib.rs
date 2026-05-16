//! Thrum wire protocol — source of truth in Rust.
//!
//! The thrum is the bidirectional NDJSON socket between humd and any
//! nestling. Every frame is a *tone*: an envelope plus a chi-specific
//! body. This crate enumerates the chi registry, defines the envelope,
//! and provides the small primitives (sigil, rid, wane, dusk, echo)
//! every other crate leans on.

mod chi;
mod envelope;
mod prims;
mod wane;

pub use chi::{Chi, PulseKind};
pub use envelope::{Envelope, Tone};
pub use prims::{dusk_in, echo_for, is_dusk, now_ms, rid, sigil};
pub use wane::WaneTracker;

/// Protocol semver — independent of any package version.
///
/// Bump rules:
///   - patch: docstring tweaks, optional fields added with safe defaults
///   - minor: new chi value, new required field with backward-compat path
///   - major: removed chi, renamed chi, removed field, changed semantics
pub const THRUM_VERSION: &str = "0.3.0";

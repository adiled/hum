//! Thrum wire protocol — source of truth in Rust.
//!
//! The thrum is the bidirectional NDJSON socket between humd and any
//! bee. Every frame is a *tone*: an envelope plus a chi-specific
//! body. This crate enumerates the chi registry, defines the envelope,
//! and provides the small primitives (sigil, rid, wane, dusk, echo)
//! every other crate leans on.
//!
//! [`THRUM_VERSION`] is the protocol semver — independent of any
//! package version. Bump rules:
//! - patch: docstring tweaks, optional fields added with safe defaults
//! - minor: new chi value, new required field with a backward-compat path
//! - major: removed chi, renamed chi, removed field, changed semantics

mod chi;
mod envelope;
mod prims;
mod spec;
mod views;
mod wane;

pub use chi::{Chi, PulseKind};
pub use envelope::{Envelope, Tone};
pub use prims::{dusk_in, echo_for, is_dusk, now_ms, rid, sigil};
pub use views::*;
pub use spec::{body_keys, envelope_keys, undeclared};
pub use wane::WaneTracker;

pub const THRUM_VERSION: &str = "0.8.0";

//! `hum-thrum`: the thrum client, the one every bee uses whatever it is
//! written in.
//!
//! Three layers, one behaviour:
//!
//! - [`core`]: the client with no I/O. Every decision about a bee's
//!   connection to humd (the signed hello, humd's verdict, reconnecting,
//!   the spec check on what the bee sends, advisories) is made here, and
//!   only here.
//! - [`drive`]: the core over tokio and a Unix socket. A Rust bee holds a
//!   [`Bee`] from [`Bee::link`].
//! - [`link`]: the same [`Bee`] on stdio, run as `hum bee link`. A bee in
//!   TS, Python, Go or C spawns it, and its generated client is the other
//!   end of the pipes, so every language gets the same client because
//!   there is only one.
//!
//! The core takes its [`Endpoint`] as a parameter. Today a bee reaches
//! its own humd over the Unix socket; another transport would be another
//! endpoint and another driver arm, and no bee would change.

pub mod core;
pub mod drive;
pub mod link;

pub use crate::core::{
    Advisory, BACKOFF_FIRST, BACKOFF_MOST, CLIENT_OWNED, Core, Endpoint, Output, QUIET_TRIES,
    Timer, VERDICT,
};
pub use drive::{Bee, Event, Options, Refused, Sender};
pub use link::LINK_VERSION;

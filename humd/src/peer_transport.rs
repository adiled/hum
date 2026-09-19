//! Ensemble peer transports — outbound dial + inbound accept, per wire.
//!
//! humd carries two transport flavours:
//!
//! - [`iroh`] — QUIC + Ed25519 NodeId, the production WAN path. NodeId
//!   is pinned to the daemon's [`ensemble::HumdKey`] so the signed
//!   hello verifies against the same pubkey iroh routes by.
//!
//! - [`tcp`] — plain NDJSON over TCP, the LAN / loopback path. Cheap,
//!   trust-on-first-use behind the signed hello (no transport-level
//!   crypto). Gated on `humd.tcpListen` — set to a `host:port` to
//!   accept inbound, omit to dial-only.
//!
//! Both modules expose the same shape: `dial_all` for outbound,
//! `spawn_listener` for inbound. Each accepted or dialed connection
//! lands at [`ensemble::Ensemble::install`] (signed) regardless of
//! transport, so the rest of humd sees one peer registry.
//!
//! Each transport's listener (or bind, for iroh) returns the hints a
//! remote peer would paste into its `peers.json` to dial back. humd
//! flattens those into [`hum_paths::RuntimeInfo::ensemble_addrs`].

pub(crate) mod iroh;
pub(crate) mod tcp;

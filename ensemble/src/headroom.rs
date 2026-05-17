//! Per-humd resource headroom — what peers learn from each other.
//!
//! Stub filled in by Tier 2 agent. Defines `RoostHeadroom` (free slots,
//! pressure tier, p95 latency) and a serializable representation that
//! lives inside `PeerCapabilities` and rides on every drone-beat gossip
//! frame. Lets the ensemble route away from saturated humds without
//! waiting for hard failure.

// TODO: RoostHeadroom struct, Pressure enum, PeerCapabilities.headroom wire-in.

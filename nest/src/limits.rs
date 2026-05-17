//! Per-roost OS-level resource caps — RSS, fds, CPU shares, wall-clock TTL.
//!
//! Stub filled in by Tier 3 agent. Defines the `ResourceLimits` struct
//! plumbed through `SpawnSpec`; concrete Perch impls apply via rlimit /
//! cgroups / equivalent.

// TODO: ResourceLimits struct, apply_to_child fn, SpawnSpec wiring.

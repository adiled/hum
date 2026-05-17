//! Pool-wide pressure tiers + eviction policy.
//!
//! Stub filled in by Tier 5 agent. Replaces the flat `idle_timeout` with
//! tiered behavior: Cool → Warm → Hot → Refuse based on slot occupancy
//! and aggregate roost pressure. Drives eviction order + the ensemble
//! advertise's pressure_state field.

// TODO: NestHealth enum, derive(&Nest) -> NestHealth, eviction policy.

//! Per-roost soft caps — tokens per turn / day, tool-call rate.
//!
//! Stub filled in by Tier 4 agent. The drone tracks `tokens_burned`
//! per-sigil; this module wraps that signal into a refuse-prompt gate
//! that emits `chi:"error"` with `code:"budget"` when limits would be
//! exceeded.

// TODO: Budget struct, BudgetTracker, check_admit fn.

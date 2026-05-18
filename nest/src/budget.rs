//! Per-cell soft caps — tokens per turn / day, tool-call rate.
//!
//! The drone tracks `tokens_burned` per-sigil; this module wraps that
//! signal into a refuse-prompt gate that emits `chi:"error"` with
//! `code:"budget"` when limits would be exceeded.
//!
//! Pure data + arithmetic — no IO, no clocks beyond `SystemTime::now()`.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

const DAY_MS: i64 = 86_400_000;
const MINUTE_MS: i64 = 60_000;

/// Configurable soft limits on a cell's consumption. `None` means
/// "no cap for this dimension."
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Budget {
    /// Tokens (input + output) allowed in a single turn.
    pub tokens_per_turn: Option<u64>,
    /// Tokens allowed across a rolling 24h window.
    pub tokens_per_day: Option<u64>,
    /// Tool calls allowed per rolling 60s window.
    pub tool_calls_per_minute: Option<u32>,
}

impl Budget {
    pub fn unlimited() -> Self {
        Self::default()
    }

    pub fn modest() -> Self {
        Self {
            tokens_per_turn: Some(8_000),
            tokens_per_day: Some(1_000_000),
            tool_calls_per_minute: Some(60),
        }
    }
}

/// Reason a budget check refused a new prompt or tool-call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum BudgetDenial {
    TokensPerTurn { requested: u64, cap: u64 },
    TokensPerDay { used: u64, cap: u64 },
    ToolCallsPerMinute { used: u32, cap: u32 },
}

impl BudgetDenial {
    /// Short, human-readable label — what goes into `chi:"error".message`.
    pub fn message(&self) -> String {
        match self {
            BudgetDenial::TokensPerTurn { requested, cap } => format!(
                "budget: tokens_per_turn exceeded (requested {requested}, cap {cap})"
            ),
            BudgetDenial::TokensPerDay { used, cap } => {
                format!("budget: tokens_per_day exceeded (used {used}, cap {cap})")
            }
            BudgetDenial::ToolCallsPerMinute { used, cap } => format!(
                "budget: tool_calls_per_minute exceeded (used {used}, cap {cap})"
            ),
        }
    }
}

/// Per-key running counters with windowed expiry. Cheap to clone — state
/// lives behind an `Arc<Mutex<Inner>>`.
#[derive(Debug, Clone, Default)]
pub struct BudgetTracker {
    inner: std::sync::Arc<Mutex<Inner>>,
}

#[derive(Debug, Default)]
struct Inner {
    keys: HashMap<String, KeyState>,
}

#[derive(Debug, Default)]
struct KeyState {
    /// Token grants this turn (zeroed by `note_turn_end`).
    tokens_this_turn: u64,
    /// Rolling 24h: (ts_ms, tokens) entries; pruned on each touch.
    tokens_day: Vec<(i64, u64)>,
    /// Rolling 60s: ts_ms of each tool-call; pruned on each touch.
    tool_calls_minute: Vec<i64>,
}

impl KeyState {
    fn prune_day(&mut self, now_ms: i64) {
        let cutoff = now_ms - DAY_MS;
        self.tokens_day.retain(|(ts, _)| *ts > cutoff);
    }

    fn prune_minute(&mut self, now_ms: i64) {
        let cutoff = now_ms - MINUTE_MS;
        self.tool_calls_minute.retain(|ts| *ts > cutoff);
    }

    fn day_total(&self) -> u64 {
        self.tokens_day.iter().map(|(_, t)| *t).sum()
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl BudgetTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Check whether `key` may admit a new turn of approximately `est_tokens`.
    /// `Ok(())` = admit; `Err(BudgetDenial)` = reason it was refused. The
    /// check does NOT mutate state — callers that proceed must record the
    /// turn via `record_tokens` so it counts against the rolling windows.
    pub fn check_admit_turn(
        &self,
        key: &str,
        est_tokens: u64,
        budget: &Budget,
    ) -> Result<(), BudgetDenial> {
        let now = now_ms();
        let mut inner = self.inner.lock();
        let state = inner.keys.entry(key.to_string()).or_default();
        state.prune_day(now);

        if let Some(cap) = budget.tokens_per_turn {
            if state.tokens_this_turn.saturating_add(est_tokens) > cap {
                return Err(BudgetDenial::TokensPerTurn {
                    requested: est_tokens,
                    cap,
                });
            }
        }

        if let Some(cap) = budget.tokens_per_day {
            let used = state.day_total();
            if used.saturating_add(est_tokens) > cap {
                return Err(BudgetDenial::TokensPerDay { used, cap });
            }
        }

        Ok(())
    }

    /// Check whether `key` may admit a new tool-call right now.
    pub fn check_admit_tool_call(
        &self,
        key: &str,
        budget: &Budget,
    ) -> Result<(), BudgetDenial> {
        let now = now_ms();
        let mut inner = self.inner.lock();
        let state = inner.keys.entry(key.to_string()).or_default();
        state.prune_minute(now);

        if let Some(cap) = budget.tool_calls_per_minute {
            let used = state.tool_calls_minute.len() as u32;
            if used.saturating_add(1) > cap {
                return Err(BudgetDenial::ToolCallsPerMinute { used, cap });
            }
        }

        Ok(())
    }

    /// Record `tokens` consumed against `key` — call from the drone's
    /// TextDelta observer (or in batch on chi:"finish").
    pub fn record_tokens(&self, key: &str, tokens: u64) {
        let now = now_ms();
        let mut inner = self.inner.lock();
        let state = inner.keys.entry(key.to_string()).or_default();
        state.prune_day(now);
        state.tokens_this_turn = state.tokens_this_turn.saturating_add(tokens);
        state.tokens_day.push((now, tokens));
    }

    /// Record one tool-call against `key`'s 60s window.
    pub fn record_tool_call(&self, key: &str) {
        let now = now_ms();
        let mut inner = self.inner.lock();
        let state = inner.keys.entry(key.to_string()).or_default();
        state.prune_minute(now);
        state.tool_calls_minute.push(now);
    }

    /// Reset this turn's tokens counter (call on chi:"finish").
    pub fn note_turn_end(&self, key: &str) {
        let mut inner = self.inner.lock();
        if let Some(state) = inner.keys.get_mut(key) {
            state.tokens_this_turn = 0;
        }
    }

    /// Drop the entire state for `key`. Call on chi:"cleanup".
    pub fn forget(&self, key: &str) {
        let mut inner = self.inner.lock();
        inner.keys.remove(key);
    }

    /// Test-only: record a tool call with an explicit timestamp so tests
    /// can plant entries in the past without sleeping.
    #[cfg(test)]
    pub(crate) fn record_tool_call_at(&self, key: &str, ts_ms: i64) {
        let mut inner = self.inner.lock();
        let state = inner.keys.entry(key.to_string()).or_default();
        state.tool_calls_minute.push(ts_ms);
    }
}

/// Build the body of a `chi:"error"` tone signaling a budget denial.
/// The shape: `{ "code": "budget", "message": "...", "denial": <BudgetDenial> }`.
pub fn deny_error_body(denial: &BudgetDenial) -> serde_json::Value {
    serde_json::json!({
        "code": "budget",
        "message": denial.message(),
        "denial": denial,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_budget_is_unlimited() {
        let tracker = BudgetTracker::new();
        let budget = Budget::default();
        assert!(tracker.check_admit_turn("k", 1_000_000, &budget).is_ok());
    }

    #[test]
    fn tokens_per_turn_cap_refuses() {
        let tracker = BudgetTracker::new();
        let budget = Budget {
            tokens_per_turn: Some(100),
            ..Default::default()
        };
        let denial = tracker
            .check_admit_turn("k", 200, &budget)
            .expect_err("should refuse");
        assert_eq!(
            denial,
            BudgetDenial::TokensPerTurn {
                requested: 200,
                cap: 100,
            }
        );
    }

    #[test]
    fn tokens_per_day_cap_refuses_after_accumulating() {
        let tracker = BudgetTracker::new();
        let budget = Budget {
            tokens_per_day: Some(1000),
            ..Default::default()
        };
        tracker.record_tokens("k", 800);
        let denial = tracker
            .check_admit_turn("k", 300, &budget)
            .expect_err("should refuse");
        assert_eq!(
            denial,
            BudgetDenial::TokensPerDay {
                used: 800,
                cap: 1000,
            }
        );
    }

    #[test]
    fn tool_calls_minute_cap_refuses() {
        let tracker = BudgetTracker::new();
        let budget = Budget {
            tool_calls_per_minute: Some(2),
            ..Default::default()
        };
        tracker.record_tool_call("k");
        tracker.record_tool_call("k");
        let denial = tracker
            .check_admit_tool_call("k", &budget)
            .expect_err("should refuse");
        assert!(matches!(
            denial,
            BudgetDenial::ToolCallsPerMinute { used: 2, cap: 2 }
        ));
    }

    #[test]
    fn pruning_recovers_capacity() {
        let tracker = BudgetTracker::new();
        let budget = Budget {
            tool_calls_per_minute: Some(1),
            ..Default::default()
        };
        // Plant a tool-call 70s in the past — outside the rolling window.
        let stale_ts = now_ms() - 70_000;
        tracker.record_tool_call_at("k", stale_ts);
        // The check should prune the stale entry and admit.
        assert!(tracker.check_admit_tool_call("k", &budget).is_ok());
    }

    #[test]
    fn denial_serializes_as_expected_chi_error_body() {
        let denial = BudgetDenial::TokensPerTurn {
            requested: 9000,
            cap: 8000,
        };
        let body = deny_error_body(&denial);
        assert_eq!(body["code"], "budget");
        assert!(
            body["message"]
                .as_str()
                .unwrap()
                .contains("tokens_per_turn"),
            "message should reference tokens_per_turn, got: {}",
            body["message"]
        );
        assert_eq!(body["denial"]["kind"], "tokens-per-turn");
        assert_eq!(body["denial"]["requested"], 9000);
        assert_eq!(body["denial"]["cap"], 8000);
    }
}

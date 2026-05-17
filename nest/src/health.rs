//! Pool-wide pressure tiers + eviction policy.
//!
//! Replaces a flat `idle_timeout` with a tiered classification (Cool → Warm →
//! Hot → Refuse) computed from a [`NestSnapshot`]. The classification drives
//! both local eviction order and the `pressure_state` field humd advertises
//! to the ensemble.
//!
//! Everything in this module is a pure function over plain data so the
//! policy is unit-testable without spinning up a real pool. The pool will
//! build a [`NestSnapshot`] on demand and call [`plan_evictions`] /
//! [`NestSnapshot::health`].

use serde::{Deserialize, Serialize};

/// Pool-wide pressure tier. Mirrors `ensemble::headroom::Pressure`
/// semantically but lives here because the nest crate computes it from local
/// state and passes it up; the ensemble crate's enum is the wire-side
/// representation.
///
/// Thresholds (slot occupancy):
///   Cool   — < 50%   — only idle eviction; no pressure
///   Warm   — 50-80%  — idle eviction + watch latency
///   Hot    — 80-95%  — idle eviction + LRU on non-active sigils
///   Refuse — ≥ 95%   — refuse new prompts; aggressive idle + LRU; emit signal
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NestHealth {
    Cool,
    Warm,
    Hot,
    Refuse,
}

impl NestHealth {
    /// Compute from a slot-occupancy ratio in `[0.0, 1.0]`. NaN / out-of-range
    /// values clamp; an empty nest is reported as `Cool` by the caller (this
    /// function never sees `total = 0`).
    pub fn from_occupancy(ratio: f32) -> Self {
        // NaN propagates through comparisons as false, so explicit guard.
        let r = if ratio.is_nan() {
            0.0
        } else {
            ratio.clamp(0.0, 1.0)
        };
        if r < 0.5 {
            NestHealth::Cool
        } else if r < 0.8 {
            NestHealth::Warm
        } else if r < 0.95 {
            NestHealth::Hot
        } else {
            NestHealth::Refuse
        }
    }

    /// True iff humd should refuse new prompts at this tier.
    pub fn refusing(self) -> bool {
        matches!(self, NestHealth::Refuse)
    }

    /// True iff humd may take new prompts comfortably.
    pub fn accepting(self) -> bool {
        matches!(self, NestHealth::Cool | NestHealth::Warm)
    }
}

impl Default for NestHealth {
    fn default() -> Self {
        NestHealth::Cool
    }
}

/// Plain-data snapshot of the pool the eviction policy reads. The actual
/// `Nest` struct (in `pool.rs`) builds one of these on demand. Decoupled so
/// the policy is testable without spinning up a real pool.
#[derive(Debug, Clone, Default)]
pub struct NestSnapshot {
    /// `max_procs` from `NestConfig`.
    pub total_slots: u32,
    /// One entry per currently-resident sid. Order is irrelevant.
    pub slots: Vec<SlotSnapshot>,
}

/// One resident sid's view of the world for the eviction policy.
#[derive(Debug, Clone)]
pub struct SlotSnapshot {
    pub sid: String,
    /// True if this slot has an active listener / is mid-turn.
    pub active: bool,
    /// Last-touched timestamp in ms-since-epoch.
    pub last_touched_ms: i64,
    /// True if the roost is ephemeral (PTY / REPL) — these evict cheaply.
    pub ephemeral: bool,
}

impl NestSnapshot {
    /// Current occupancy ratio in `[0.0, 1.0]`. Returns `0.0` if
    /// `total_slots == 0`.
    pub fn occupancy(&self) -> f32 {
        if self.total_slots == 0 {
            0.0
        } else {
            (self.slots.len() as f32 / self.total_slots as f32).clamp(0.0, 1.0)
        }
    }

    /// Derive the pool's health tier from occupancy. An empty pool
    /// (`total_slots == 0`) is reported as `Cool`.
    pub fn health(&self) -> NestHealth {
        if self.total_slots == 0 {
            return NestHealth::Cool;
        }
        NestHealth::from_occupancy(self.occupancy())
    }
}

/// Configuration knobs the policy reads. Lives on `NestConfig` in the real
/// pool; the snapshot includes these so the policy is a pure function.
#[derive(Debug, Clone, Copy)]
pub struct EvictionConfig {
    /// Milliseconds since `last_touched_ms` after which a slot counts as idle.
    pub idle_threshold_ms: i64,
}

impl Default for EvictionConfig {
    fn default() -> Self {
        Self {
            idle_threshold_ms: 300_000, // 5 minutes
        }
    }
}

/// Eviction target: bring occupancy down to ≤ 80% of total.
fn target_occupied(total_slots: u32) -> usize {
    // ceil(total * 0.8) — integer math: (total * 4 + 4) / 5.
    ((total_slots as usize) * 4 + 4) / 5
}

/// Iff `now_ms - last_touched_ms > idle_threshold_ms`.
fn is_idle(slot: &SlotSnapshot, now_ms: i64, cfg: EvictionConfig) -> bool {
    now_ms.saturating_sub(slot.last_touched_ms) > cfg.idle_threshold_ms
}

/// Decide which sids to evict at the current health tier. Returns sids in
/// the order they should be killed — caller stops early if pressure clears.
///
/// Policy by tier:
///   Cool   — only sids whose `last_touched_ms` is older than
///            `idle_threshold_ms`.
///   Warm   — same as Cool. (Pressure isn't high enough to evict mid-turn.)
///   Hot    — idle sids + LRU on non-active (inactive) sids until under 80%.
///   Refuse — every ephemeral + every non-active, sorted by oldest first,
///            until under 80%. Active sids are LAST resort.
///
/// `now_ms` is passed in (not read from the system clock) so the policy is
/// deterministic and testable with explicit timestamps.
pub fn plan_evictions(
    snap: &NestSnapshot,
    cfg: EvictionConfig,
    now_ms: i64,
) -> Vec<String> {
    if snap.slots.is_empty() || snap.total_slots == 0 {
        return Vec::new();
    }

    let health = snap.health();
    let target = target_occupied(snap.total_slots);
    let occupied = snap.slots.len();

    // Indices over `snap.slots` so we can preserve original ordering when
    // sort keys tie (stable sort below relies on this).
    let mut idle_indices: Vec<usize> = (0..snap.slots.len())
        .filter(|&i| is_idle(&snap.slots[i], now_ms, cfg))
        .collect();
    // Idle list ordered oldest-first so the most-stale sids die first.
    idle_indices.sort_by_key(|&i| snap.slots[i].last_touched_ms);

    let mut plan: Vec<String> = Vec::new();
    let mut taken: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut projected = occupied;

    // 1. Idle eviction — applies at every tier.
    for i in idle_indices {
        plan.push(snap.slots[i].sid.clone());
        taken.insert(i);
        projected = projected.saturating_sub(1);
    }

    match health {
        NestHealth::Cool | NestHealth::Warm => {
            // No additional pressure-based eviction. Idle-only.
        }
        NestHealth::Hot => {
            // LRU on non-active, non-idle slots until projected ≤ target.
            if projected > target {
                let mut non_active: Vec<usize> = (0..snap.slots.len())
                    .filter(|&i| !taken.contains(&i) && !snap.slots[i].active)
                    .collect();
                non_active.sort_by_key(|&i| snap.slots[i].last_touched_ms);
                for i in non_active {
                    if projected <= target {
                        break;
                    }
                    plan.push(snap.slots[i].sid.clone());
                    taken.insert(i);
                    projected -= 1;
                }
            }
        }
        NestHealth::Refuse => {
            // Ephemeral first (regardless of active), then non-active LRU,
            // then active LRU as last resort. All ordered oldest-first
            // within each band.
            if projected > target {
                let mut ephemerals: Vec<usize> = (0..snap.slots.len())
                    .filter(|&i| !taken.contains(&i) && snap.slots[i].ephemeral)
                    .collect();
                ephemerals.sort_by_key(|&i| snap.slots[i].last_touched_ms);
                for i in ephemerals {
                    if projected <= target {
                        break;
                    }
                    plan.push(snap.slots[i].sid.clone());
                    taken.insert(i);
                    projected -= 1;
                }
            }
            if projected > target {
                let mut non_active: Vec<usize> = (0..snap.slots.len())
                    .filter(|&i| !taken.contains(&i) && !snap.slots[i].active)
                    .collect();
                non_active.sort_by_key(|&i| snap.slots[i].last_touched_ms);
                for i in non_active {
                    if projected <= target {
                        break;
                    }
                    plan.push(snap.slots[i].sid.clone());
                    taken.insert(i);
                    projected -= 1;
                }
            }
            if projected > target {
                let mut actives: Vec<usize> = (0..snap.slots.len())
                    .filter(|&i| !taken.contains(&i) && snap.slots[i].active)
                    .collect();
                actives.sort_by_key(|&i| snap.slots[i].last_touched_ms);
                for i in actives {
                    if projected <= target {
                        break;
                    }
                    plan.push(snap.slots[i].sid.clone());
                    taken.insert(i);
                    projected -= 1;
                }
            }
        }
    }

    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(sid: &str, active: bool, last: i64, ephemeral: bool) -> SlotSnapshot {
        SlotSnapshot {
            sid: sid.to_string(),
            active,
            last_touched_ms: last,
            ephemeral,
        }
    }

    #[test]
    fn health_threshold_bands() {
        // Below 0.5 → Cool.
        assert_eq!(NestHealth::from_occupancy(0.0), NestHealth::Cool);
        assert_eq!(NestHealth::from_occupancy(0.49), NestHealth::Cool);
        // At 0.5 → Warm.
        assert_eq!(NestHealth::from_occupancy(0.5), NestHealth::Warm);
        assert_eq!(NestHealth::from_occupancy(0.79), NestHealth::Warm);
        // At 0.8 → Hot.
        assert_eq!(NestHealth::from_occupancy(0.8), NestHealth::Hot);
        assert_eq!(NestHealth::from_occupancy(0.94), NestHealth::Hot);
        // At 0.95 → Refuse.
        assert_eq!(NestHealth::from_occupancy(0.95), NestHealth::Refuse);
        assert_eq!(NestHealth::from_occupancy(1.0), NestHealth::Refuse);

        // Clamp behavior.
        assert_eq!(NestHealth::from_occupancy(-0.1), NestHealth::Cool);
        assert_eq!(NestHealth::from_occupancy(2.0), NestHealth::Refuse);
        assert_eq!(NestHealth::from_occupancy(f32::NAN), NestHealth::Cool);
    }

    #[test]
    fn cool_only_evicts_idle() {
        // 3 of 10 slots → 30% → Cool. Two old, one fresh.
        let now = 10_000_i64;
        let snap = NestSnapshot {
            total_slots: 10,
            slots: vec![
                slot("old-a", false, 1_000, false),  // 9s old
                slot("old-b", false, 2_000, false),  // 8s old
                slot("fresh", false, 9_500, false),  // 0.5s old
            ],
        };
        assert_eq!(snap.health(), NestHealth::Cool);

        let cfg = EvictionConfig {
            idle_threshold_ms: 1_000,
        };
        let plan = plan_evictions(&snap, cfg, now);

        assert_eq!(plan.len(), 2);
        assert!(plan.contains(&"old-a".to_string()));
        assert!(plan.contains(&"old-b".to_string()));
        assert!(!plan.contains(&"fresh".to_string()));
        // Stable LRU order: oldest first.
        assert_eq!(plan, vec!["old-a", "old-b"]);
    }

    #[test]
    fn warm_behaves_like_cool() {
        // 5 of 10 slots → 50% → Warm. Two idle, three fresh.
        let now = 10_000_i64;
        let snap = NestSnapshot {
            total_slots: 10,
            slots: vec![
                slot("old-a", false, 1_000, false),
                slot("old-b", false, 2_000, false),
                slot("fresh-1", false, 9_500, false),
                slot("fresh-2", false, 9_600, false),
                slot("fresh-3", false, 9_700, false),
            ],
        };
        assert_eq!(snap.health(), NestHealth::Warm);

        let cfg = EvictionConfig {
            idle_threshold_ms: 1_000,
        };
        let plan = plan_evictions(&snap, cfg, now);

        // Warm: idle only, no LRU pressure.
        assert_eq!(plan, vec!["old-a", "old-b"]);
    }

    #[test]
    fn hot_adds_lru_for_inactive() {
        // 10 of 10 slots → 100% → Refuse tier exercises the non-active-LRU
        // path (same path the Hot tier uses). None are idle, all inactive,
        // no ephemerals. Target = ceil(10 * 0.8) = 8, so 2 evictions; the
        // two oldest must come first.
        let now = 10_000_i64;
        let mut slots = Vec::new();
        for i in 0..10 {
            // last_touched_ms = now (fresh), but per-sid offset so LRU is
            // unambiguous: lower i = older.
            slots.push(slot(
                &format!("s{i}"),
                false,
                now - (10 - i as i64), // s0 oldest, s9 newest
                false,
            ));
        }
        let snap = NestSnapshot {
            total_slots: 10,
            slots,
        };
        assert_eq!(snap.health(), NestHealth::Refuse);

        // Threshold high enough that no slot counts as "idle". This forces
        // the LRU-for-inactive path, not the idle path.
        let cfg = EvictionConfig {
            idle_threshold_ms: 60_000,
        };
        let plan = plan_evictions(&snap, cfg, now);

        // Exactly two evictions: oldest first.
        assert_eq!(plan.len(), 2);
        assert_eq!(plan, vec!["s0".to_string(), "s1".to_string()]);
    }

    #[test]
    fn refuse_prefers_ephemeral() {
        // 10 of 10 slots, all active, none idle. Two ephemeral. Target = 8,
        // so 2 evictions; ephemerals come first regardless of LRU order
        // among non-ephemerals.
        let now = 10_000_i64;
        let mut slots = Vec::new();
        for i in 0..10 {
            // All very recent; ephemeral on indices 4 and 7.
            let eph = i == 4 || i == 7;
            slots.push(slot(
                &format!("s{i}"),
                true, // all active
                now - (10 - i as i64),
                eph,
            ));
        }
        let snap = NestSnapshot {
            total_slots: 10,
            slots,
        };
        assert_eq!(snap.health(), NestHealth::Refuse);

        let cfg = EvictionConfig {
            idle_threshold_ms: 60_000,
        };
        let plan = plan_evictions(&snap, cfg, now);

        assert_eq!(plan.len(), 2);
        // Ephemerals come first; among them, the older one (s4) precedes s7.
        assert_eq!(plan, vec!["s4".to_string(), "s7".to_string()]);
    }

    #[test]
    fn empty_snapshot_yields_empty_plan() {
        let snap = NestSnapshot::default();
        assert_eq!(snap.total_slots, 0);
        assert!(snap.slots.is_empty());
        assert_eq!(snap.occupancy(), 0.0);
        assert_eq!(snap.health(), NestHealth::Cool);

        let plan = plan_evictions(&snap, EvictionConfig::default(), 0);
        assert!(plan.is_empty());
    }
}

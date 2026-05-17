//! Per-humd resource headroom — what peers learn from each other.
//!
//! Defines `RoostHeadroom` (free slots, pressure tier, p95 latency) and a
//! serializable representation that lives inside `PeerCapabilities` and
//! rides on every drone-beat gossip frame. Lets the ensemble route away
//! from saturated humds without waiting for hard failure.

use serde::{Deserialize, Serialize};

/// Coarse pressure tier — what the ensemble advertise lets peers see
/// without exposing internal metrics. Maps roughly to:
///   Cool   — < 50% slot occupancy, plenty of headroom
///   Warm   — 50–80%, still accepting prompts comfortably
///   Hot    — 80–95%, prefer to route elsewhere if possible
///   Refuse — > 95%, do not route new prompts here (humd will reject)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Pressure {
    Cool,
    Warm,
    Hot,
    Refuse,
}

impl Default for Pressure {
    fn default() -> Self {
        // No-op advertise — an empty humd is at rest, not refusing.
        Pressure::Cool
    }
}

impl Pressure {
    /// Compute from a slot-occupancy ratio (0.0 .. 1.0). Out-of-range
    /// values clamp.
    pub fn from_occupancy(ratio: f32) -> Self {
        // Clamp first — NaN-safe via clamp's contract: if ratio is NaN
        // clamp returns NaN, but f32::clamp panics on NaN bounds; the
        // bounds here are finite so we only need to worry about NaN
        // input. Treat NaN as Cool by falling through all branches.
        let r = ratio.clamp(0.0, 1.0);
        if r < 0.5 {
            Pressure::Cool
        } else if r < 0.8 {
            Pressure::Warm
        } else if r < 0.95 {
            Pressure::Hot
        } else {
            Pressure::Refuse
        }
    }

    /// True if a peer should *avoid* routing a new prompt here.
    pub fn is_refusing(self) -> bool {
        matches!(self, Pressure::Refuse)
    }
}

/// Live snapshot of a humd's roost-pool capacity, gossiped to peer humds.
/// Empty values mean "this humd has no nest" (still a valid mesh peer —
/// it can route but cannot answer).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoostHeadroom {
    /// Free roost slots = max_procs - current_roost_count.
    pub free_slots: u32,
    /// Total slots (the max_procs config).
    pub total_slots: u32,
    /// p95 turn latency in ms, if the drift crate has been recording.
    /// None if no measurements yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p95_latency_ms: Option<u32>,
    /// Derived pressure tier — what peers branch on.
    pub pressure: Pressure,
}

impl RoostHeadroom {
    /// Build from raw pool counts. Computes pressure from occupancy.
    ///
    /// When `total_slots == 0` (no nest), pressure is `Cool` and
    /// `free_slots` is forced to 0 — the humd is honest about having
    /// nothing to offer rather than misrepresenting itself as full.
    pub fn from_counts(
        free_slots: u32,
        total_slots: u32,
        p95_latency_ms: Option<u32>,
    ) -> Self {
        if total_slots == 0 {
            return Self {
                free_slots: 0,
                total_slots: 0,
                p95_latency_ms,
                pressure: Pressure::Cool,
            };
        }
        // Saturate free_slots at total_slots so a stale count can't
        // produce a negative occupancy.
        let free = free_slots.min(total_slots);
        let occupied = total_slots - free;
        let ratio = occupied as f32 / total_slots as f32;
        Self {
            free_slots: free,
            total_slots,
            p95_latency_ms,
            pressure: Pressure::from_occupancy(ratio),
        }
    }

    /// Build the "empty" advertise — a humd with no nest. free=0, total=0,
    /// pressure=Cool (it's not refusing anything because it can't answer
    /// anything to begin with).
    pub fn empty() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_thresholds_match_docstring() {
        // Cool: below 0.5
        assert_eq!(Pressure::from_occupancy(0.0), Pressure::Cool);
        assert_eq!(Pressure::from_occupancy(0.25), Pressure::Cool);
        assert_eq!(Pressure::from_occupancy(0.49), Pressure::Cool);
        // Boundary at 0.5 — `<` not `<=` means 0.5 is Warm.
        assert_eq!(Pressure::from_occupancy(0.5), Pressure::Warm);
        assert_eq!(Pressure::from_occupancy(0.65), Pressure::Warm);
        assert_eq!(Pressure::from_occupancy(0.79), Pressure::Warm);
        // Boundary at 0.8 — Hot starts at exactly 0.8.
        assert_eq!(Pressure::from_occupancy(0.8), Pressure::Hot);
        assert_eq!(Pressure::from_occupancy(0.9), Pressure::Hot);
        assert_eq!(Pressure::from_occupancy(0.94), Pressure::Hot);
        // Boundary at 0.95 — Refuse starts at exactly 0.95.
        assert_eq!(Pressure::from_occupancy(0.95), Pressure::Refuse);
        assert_eq!(Pressure::from_occupancy(0.99), Pressure::Refuse);
        assert_eq!(Pressure::from_occupancy(1.0), Pressure::Refuse);
    }

    #[test]
    fn clamp_handles_extreme_ratios() {
        assert_eq!(Pressure::from_occupancy(-1.0), Pressure::Cool);
        assert_eq!(Pressure::from_occupancy(-100.0), Pressure::Cool);
        assert_eq!(Pressure::from_occupancy(2.0), Pressure::Refuse);
        assert_eq!(Pressure::from_occupancy(1000.0), Pressure::Refuse);
    }

    #[test]
    fn from_counts_computes_pressure() {
        // 5 free of 10 → 50% occupied → Warm.
        let h = RoostHeadroom::from_counts(5, 10, None);
        assert_eq!(h.free_slots, 5);
        assert_eq!(h.total_slots, 10);
        assert_eq!(h.pressure, Pressure::Warm);
        assert_eq!(h.p95_latency_ms, None);

        // 9 free of 10 → 10% occupied → Cool.
        let cool = RoostHeadroom::from_counts(9, 10, Some(120));
        assert_eq!(cool.pressure, Pressure::Cool);
        assert_eq!(cool.p95_latency_ms, Some(120));

        // 0 free of 10 → 100% occupied → Refuse.
        let refuse = RoostHeadroom::from_counts(0, 10, None);
        assert_eq!(refuse.pressure, Pressure::Refuse);
        assert!(refuse.pressure.is_refusing());

        // 1 free of 10 → 90% occupied → Hot.
        let hot = RoostHeadroom::from_counts(1, 10, None);
        assert_eq!(hot.pressure, Pressure::Hot);
    }

    #[test]
    fn empty_humd_is_cool() {
        let e = RoostHeadroom::empty();
        assert_eq!(e.pressure, Pressure::Cool);
        assert_eq!(e.free_slots, 0);
        assert_eq!(e.total_slots, 0);
        assert_eq!(e.p95_latency_ms, None);
        assert!(!e.pressure.is_refusing());

        // from_counts with total_slots=0 behaves the same.
        let zero = RoostHeadroom::from_counts(0, 0, None);
        assert_eq!(zero.pressure, Pressure::Cool);
        assert_eq!(zero.free_slots, 0);
        assert_eq!(zero.total_slots, 0);

        // Stale count where free > total=0 is still honest.
        let stale = RoostHeadroom::from_counts(7, 0, None);
        assert_eq!(stale.free_slots, 0);
        assert_eq!(stale.total_slots, 0);
        assert_eq!(stale.pressure, Pressure::Cool);
    }

    #[test]
    fn roundtrips_through_json() {
        let cases = vec![
            RoostHeadroom::empty(),
            RoostHeadroom::from_counts(5, 10, None),
            RoostHeadroom::from_counts(2, 16, Some(450)),
            RoostHeadroom::from_counts(0, 4, Some(9999)),
            RoostHeadroom::from_counts(64, 64, Some(12)),
        ];
        for original in cases {
            let s = serde_json::to_string(&original).expect("serialize");
            let back: RoostHeadroom = serde_json::from_str(&s).expect("deserialize");
            assert_eq!(back.free_slots, original.free_slots);
            assert_eq!(back.total_slots, original.total_slots);
            assert_eq!(back.p95_latency_ms, original.p95_latency_ms);
            assert_eq!(back.pressure, original.pressure);
        }

        // Spot-check kebab-case serialization of the enum.
        let h = RoostHeadroom::from_counts(0, 10, None);
        let s = serde_json::to_string(&h).unwrap();
        assert!(s.contains("\"refuse\""), "expected kebab-case: {}", s);

        // skip_serializing_if for None p95.
        let no_latency = RoostHeadroom::from_counts(5, 10, None);
        let s = serde_json::to_string(&no_latency).unwrap();
        assert!(!s.contains("p95_latency_ms"), "None should be skipped: {}", s);
    }
}

//! A worker's headroom: how many more cells it can take.
//!
//! A worker that hosts cells claims its headroom on its hello and on each
//! cell pulse; humd keeps the latest claim on the bee's manifest and
//! gossips it, so a prompt goes to a worker with room, here or on a peer.
//! humd itself hosts no cells and claims none.
//!
//! Pressure is a coarse occupancy tier, not a metric dump: `Cool` under
//! half the slots taken, `Warm` to 80%, `Hot` to 95%, `Refuse` above.
//! A worker at `Refuse` is passed over while another worker can serve.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum Pressure {
    #[default]
    Cool,
    Warm,
    Hot,
    Refuse,
}

impl Pressure {
    pub fn from_occupancy(ratio: f32) -> Self {
        if ratio.is_nan() {
            return Pressure::Cool;
        }
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

    pub fn is_refusing(self) -> bool {
        matches!(self, Pressure::Refuse)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CellHeadroom {
    pub free_slots: u32,
    pub total_slots: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p95_latency_ms: Option<u32>,
    pub pressure: Pressure,
}

impl CellHeadroom {
    pub fn from_counts(free_slots: u32, total_slots: u32, p95_latency_ms: Option<u32>) -> Self {
        if total_slots == 0 {
            return Self {
                free_slots: 0,
                total_slots: 0,
                p95_latency_ms,
                pressure: Pressure::Cool,
            };
        }
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

    pub fn empty() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_thresholds_match_docstring() {
        assert_eq!(Pressure::from_occupancy(0.0), Pressure::Cool);
        assert_eq!(Pressure::from_occupancy(0.25), Pressure::Cool);
        assert_eq!(Pressure::from_occupancy(0.49), Pressure::Cool);
        assert_eq!(Pressure::from_occupancy(0.5), Pressure::Warm);
        assert_eq!(Pressure::from_occupancy(0.65), Pressure::Warm);
        assert_eq!(Pressure::from_occupancy(0.79), Pressure::Warm);
        assert_eq!(Pressure::from_occupancy(0.8), Pressure::Hot);
        assert_eq!(Pressure::from_occupancy(0.9), Pressure::Hot);
        assert_eq!(Pressure::from_occupancy(0.94), Pressure::Hot);
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
    fn nan_occupancy_is_cool() {
        assert_eq!(Pressure::from_occupancy(f32::NAN), Pressure::Cool);
    }

    #[test]
    fn from_counts_computes_pressure() {
        let h = CellHeadroom::from_counts(5, 10, None);
        assert_eq!(h.free_slots, 5);
        assert_eq!(h.total_slots, 10);
        assert_eq!(h.pressure, Pressure::Warm);
        assert_eq!(h.p95_latency_ms, None);

        let cool = CellHeadroom::from_counts(9, 10, Some(120));
        assert_eq!(cool.pressure, Pressure::Cool);
        assert_eq!(cool.p95_latency_ms, Some(120));

        let refuse = CellHeadroom::from_counts(0, 10, None);
        assert_eq!(refuse.pressure, Pressure::Refuse);
        assert!(refuse.pressure.is_refusing());

        let hot = CellHeadroom::from_counts(1, 10, None);
        assert_eq!(hot.pressure, Pressure::Hot);
    }

    #[test]
    fn empty_humd_is_cool() {
        let e = CellHeadroom::empty();
        assert_eq!(e.pressure, Pressure::Cool);
        assert_eq!(e.free_slots, 0);
        assert_eq!(e.total_slots, 0);
        assert_eq!(e.p95_latency_ms, None);
        assert!(!e.pressure.is_refusing());

        let zero = CellHeadroom::from_counts(0, 0, None);
        assert_eq!(zero.pressure, Pressure::Cool);
        assert_eq!(zero.free_slots, 0);
        assert_eq!(zero.total_slots, 0);

        let stale = CellHeadroom::from_counts(7, 0, None);
        assert_eq!(stale.free_slots, 0);
        assert_eq!(stale.total_slots, 0);
        assert_eq!(stale.pressure, Pressure::Cool);
    }

    #[test]
    fn roundtrips_through_json() {
        let cases = vec![
            CellHeadroom::empty(),
            CellHeadroom::from_counts(5, 10, None),
            CellHeadroom::from_counts(2, 16, Some(450)),
            CellHeadroom::from_counts(0, 4, Some(9999)),
            CellHeadroom::from_counts(64, 64, Some(12)),
        ];
        for original in cases {
            let s = serde_json::to_string(&original).expect("serialize");
            let back: CellHeadroom = serde_json::from_str(&s).expect("deserialize");
            assert_eq!(back.free_slots, original.free_slots);
            assert_eq!(back.total_slots, original.total_slots);
            assert_eq!(back.p95_latency_ms, original.p95_latency_ms);
            assert_eq!(back.pressure, original.pressure);
        }

        let h = CellHeadroom::from_counts(0, 10, None);
        let s = serde_json::to_string(&h).unwrap();
        assert!(s.contains("\"refuse\""), "expected kebab-case: {}", s);

        let no_latency = CellHeadroom::from_counts(5, 10, None);
        let s = serde_json::to_string(&no_latency).unwrap();
        assert!(
            !s.contains("p95_latency_ms"),
            "None should be skipped: {}",
            s
        );
    }
}

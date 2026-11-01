//! WaneTracker — per-sigil monotonic drift counter.
//!
//! Both ends of the thrum keep their own wane per sigil; divergence
//! between local and remote wane is the drift signal that triggers a
//! resync. Internally guarded by a parking_lot Mutex for fast,
//! non-async access from hot paths.

use std::collections::HashMap;

use parking_lot::Mutex;

#[derive(Default)]
pub struct WaneTracker {
    counters: Mutex<HashMap<String, u64>>,
}

impl WaneTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, sigil: &str) -> u64 {
        *self.counters.lock().get(sigil).unwrap_or(&0)
    }

    pub fn tick(&self, sigil: &str) -> u64 {
        let mut g = self.counters.lock();
        let entry = g.entry(sigil.to_string()).or_insert(0);
        *entry += 1;
        *entry
    }

    pub fn set(&self, sigil: &str, value: u64) {
        self.counters.lock().insert(sigil.to_string(), value);
    }

    pub fn forget(&self, sigil: &str) {
        self.counters.lock().remove(sigil);
    }

    pub fn behind(&self, sigil: &str, remote: u64) -> bool {
        remote > self.get(sigil)
    }

    pub fn snapshot(&self) -> HashMap<String, u64> {
        self.counters.lock().clone()
    }

    pub fn merge(&self, remote: &HashMap<String, u64>) -> usize {
        let mut g = self.counters.lock();
        let mut advanced = 0;
        for (sigil, &rv) in remote {
            let slot = g.entry(sigil.clone()).or_insert(0);
            if rv > *slot {
                *slot = rv;
                advanced += 1;
            }
        }
        advanced
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_increments_per_sigil() {
        let w = WaneTracker::new();
        assert_eq!(w.get("a"), 0);
        assert_eq!(w.tick("a"), 1);
        assert_eq!(w.tick("a"), 2);
        assert_eq!(w.get("b"), 0);
    }

    #[test]
    fn behind_detects_drift() {
        let w = WaneTracker::new();
        w.set("s", 4);
        assert!(w.behind("s", 5));
        assert!(!w.behind("s", 4));
        assert!(!w.behind("s", 3));
    }

    #[test]
    fn a_forgotten_sigil_is_gone() {
        let w = WaneTracker::new();
        w.tick("a");
        w.tick("b");
        w.forget("a");
        assert_eq!(w.get("a"), 0);
        assert_eq!(w.snapshot().len(), 1);
    }
}

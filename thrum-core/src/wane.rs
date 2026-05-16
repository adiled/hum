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

    /// Current wane for `sigil`, or 0 if untracked.
    pub fn get(&self, sigil: &str) -> u64 {
        *self.counters.lock().get(sigil).unwrap_or(&0)
    }

    /// Increment and return the new wane. Call on every state mutation.
    pub fn tick(&self, sigil: &str) -> u64 {
        let mut g = self.counters.lock();
        let entry = g.entry(sigil.to_string()).or_insert(0);
        *entry += 1;
        *entry
    }

    /// Pin wane to a known value (e.g. restored from breath).
    pub fn set(&self, sigil: &str, value: u64) {
        self.counters.lock().insert(sigil.to_string(), value);
    }

    /// True when the remote wane is ahead of local — local is stale.
    pub fn behind(&self, sigil: &str, remote: u64) -> bool {
        remote > self.get(sigil)
    }

    /// Full snapshot of every tracked sigil → wane. Used by the partition
    /// reconciliation handshake — both sides exchange snapshots on heal
    /// and merge by max.
    pub fn snapshot(&self) -> HashMap<String, u64> {
        self.counters.lock().clone()
    }

    /// Merge a remote snapshot into local state: for each sigil, take the
    /// max of local and remote. Wane is a Lamport clock — max is the
    /// convergent join. Returns the count of sigils whose local value
    /// advanced.
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
}

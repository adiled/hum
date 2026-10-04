use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use tokio::sync::Notify;

/// `arm` is synchronous so it can be called from a non-async `install`.
/// Every later `send` parks until the opening frame has landed, and a
/// shut gate never reopens.
#[derive(Debug)]
pub struct Gate {
    open: Mutex<bool>,
    arrived: Notify,
    closed: AtomicBool,
}

impl Gate {
    pub fn new() -> Self {
        Self {
            open: Mutex::new(true),
            arrived: Notify::new(),
            closed: AtomicBool::new(false),
        }
    }

    pub fn arm(&self) {
        *self.open.lock() = false;
    }

    pub fn is_open(&self) -> bool {
        *self.open.lock()
    }

    pub async fn wait(&self) {
        loop {
            // Register before reading the flag: `notify_waiters` reaches
            // only waiters already enrolled, so checking first can drop a
            // wakeup and park forever.
            if self.is_open() || self.closed.load(Ordering::SeqCst) {
                return;
            }
            self.arrived.notified().await;
        }
    }

    pub fn opened(&self) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        *self.open.lock() = true;
        self.arrived.notify_waiters();
    }

    pub fn shut(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.arrived.notify_waiters();
    }
}

impl Default for Gate {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn an_armed_gate_parks_until_it_opens() {
        let gate = Arc::new(Gate::new());
        gate.arm();
        assert!(!gate.is_open());

        let waiter = tokio::spawn({
            let gate = gate.clone();
            async move {
                gate.wait().await;
                "through"
            }
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiter.is_finished(), "an armed gate must hold traffic");

        gate.opened();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), waiter)
                .await
                .unwrap()
                .unwrap(),
            "through"
        );
    }

    #[tokio::test]
    async fn a_gate_that_is_never_armed_passes_immediately() {
        let gate = Gate::new();
        tokio::time::timeout(Duration::from_millis(50), gate.wait())
            .await
            .expect("an open gate must not park");
    }

    #[tokio::test]
    async fn shutting_releases_waiters_so_a_stalled_opening_frame_cannot_wedge_a_link() {
        let gate = Arc::new(Gate::new());
        gate.arm();
        let waiter = tokio::spawn({
            let gate = gate.clone();
            async move {
                gate.wait().await;
                "released"
            }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        gate.shut();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), waiter)
                .await
                .unwrap()
                .unwrap(),
            "released"
        );
    }

    #[tokio::test]
    async fn a_late_opening_frame_cannot_resurrect_a_shut_gate() {
        let gate = Gate::new();
        gate.arm();
        gate.shut();
        gate.opened();
        assert!(!gate.is_open(), "a closed link must stay closed");
        tokio::time::timeout(Duration::from_millis(50), gate.wait())
            .await
            .expect("a shut gate must not park");
    }

    #[tokio::test]
    async fn a_default_gate_starts_open() {
        let gate = Gate::default();
        assert!(gate.is_open(), "default must match new");
    }
}

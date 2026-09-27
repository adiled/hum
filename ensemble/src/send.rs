//! Bounded peer sends.
//!
//! A peer that stops reading is not a peer that closes. The connection
//! stays up, the handshake stays valid, and the lease keeps renewing
//! from whatever else is still arriving — so nothing looks wrong until
//! the socket buffer fills and the write stops completing.
//!
//! That is the wedge. `send` awaits the write, and on most transports
//! holds the connection's write mutex while it does, so one stalled peer
//! blocks every later send to that peer. When the caller is the single
//! task that dispatches to *all* peers, one stalled peer becomes
//! mesh-wide message loss. The only symptom is a `Lagged(n)` on a
//! broadcast receiver, which reports how many tones were skipped and
//! nothing about why.
//!
//! So every send is bounded. On timeout the connection is closed rather
//! than merely retried: a peer that cannot take a write is not slow, and
//! closing it is what lets the liveness lease mark it dead and the
//! supervisor redial it. Bounding the send is what turns a silent
//! mesh-wide stall into one eviction.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::{PeerConnection, Tone};

/// How long one send may take before the peer is declared unusable.
///
/// Sized for a slow-but-alive link, not a fast one: a tone is a few KB
/// of NDJSON, so anything that has not drained in this long is not
/// draining at all. A liveness probe shares this deadline deliberately
/// — a probe that blocks forever is a probe that never reports the
/// stall it exists to find.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(5);

/// Why a bounded send failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SendError {
    /// The peer stopped reading and the write did not complete in time.
    #[error("peer stalled: no write completed within the send deadline")]
    TimedOut,
    /// The write failed outright, or the transport refused it. Keeps the
    /// transport's own message: "it failed" is not actionable and the
    /// whole reason this module exists is that failures were invisible.
    #[error("write failed: {0}")]
    Failed(String),
}

/// Sends that did not complete. A number, not a log line — a stall is
/// something to alert on, and `warn!` is what made it invisible before.
#[derive(Debug, Default)]
pub struct SendStats {
    pub timed_out: AtomicU64,
    pub failed: AtomicU64,
}

impl SendStats {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn timed_out(&self) -> u64 {
        self.timed_out.load(Ordering::SeqCst)
    }

    pub fn failed(&self) -> u64 {
        self.failed.load(Ordering::SeqCst)
    }
}

/// One send to one peer, under [`SEND_TIMEOUT`].
///
/// On timeout the connection is closed. That is the point: a stalled
/// write is not a slow write, and leaving the connection open would let
/// the lease keep calling a dead peer live.
pub async fn send_bounded(
    conn: &Arc<dyn PeerConnection>,
    tone: Tone,
    stats: &SendStats,
) -> Result<(), SendError> {
    send_bounded_with(conn, tone, stats, SEND_TIMEOUT).await
}

/// [`send_bounded`] with an explicit deadline, so tests need not wait
/// out the production timeout.
pub async fn send_bounded_with(
    conn: &Arc<dyn PeerConnection>,
    tone: Tone,
    stats: &SendStats,
    deadline: Duration,
) -> Result<(), SendError> {
    match tokio::time::timeout(deadline, conn.send(tone)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => {
            stats.failed.fetch_add(1, Ordering::SeqCst);
            Err(SendError::Failed(e.to_string()))
        }
        Err(_) => {
            stats.timed_out.fetch_add(1, Ordering::SeqCst);
            tracing::warn!(
                target: "ensemble.send",
                peer = %conn.peer().id.short(),
                timeout_ms = deadline.as_millis() as u64,
                "send.timed_out: peer stopped reading, closing"
            );
            conn.close();
            Err(SendError::TimedOut)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Hid, HumdAddr, PeerCapabilities};
    use parking_lot::Mutex;
    use std::sync::atomic::AtomicBool;

    /// A peer that accepts a connection and then stops reading — what a
    /// full socket buffer with a non-reading peer looks like from the
    /// writer's side. `stalled == true` never completes the write.
    struct Stalled {
        addr: HumdAddr,
        caps: PeerCapabilities,
        stalled: AtomicBool,
        closed: AtomicBool,
        sends: Mutex<usize>,
    }

    #[async_trait::async_trait]
    impl PeerConnection for Stalled {
        fn peer(&self) -> &HumdAddr {
            &self.addr
        }
        fn capabilities(&self) -> &PeerCapabilities {
            &self.caps
        }
        async fn send(&self, _tone: Tone) -> anyhow::Result<()> {
            *self.sends.lock() += 1;
            if self.stalled.load(Ordering::SeqCst) {
                // Never completes. This is the wedge.
                std::future::pending::<()>().await;
            }
            Ok(())
        }
        fn take_receiver(&self) -> Option<tokio::sync::mpsc::Receiver<Tone>> {
            None
        }
        fn close(&self) {
            self.closed.store(true, Ordering::SeqCst);
        }
    }

    fn conn(stalls: bool) -> Arc<Stalled> {
        Arc::new(Stalled {
            addr: HumdAddr { id: Hid::random_humd(), hints: vec![] },
            caps: PeerCapabilities::default(),
            stalled: AtomicBool::new(stalls),
            closed: AtomicBool::new(false),
            sends: Mutex::new(0),
        })
    }

    fn as_conn(c: &Arc<Stalled>) -> Arc<dyn PeerConnection> {
        c.clone()
    }

    /// The whole point: a stalled peer must not hold the caller. Run with
    /// a short deadline so the test is not paced by production's 5s.
    #[tokio::test]
    async fn a_stalled_peer_times_out_rather_than_hanging() {
        let c = conn(false);
        c.stalled.store(true, Ordering::SeqCst);
        let stats = SendStats::new();
        let short = Duration::from_millis(50);

        let started = std::time::Instant::now();
        let got = tokio::time::timeout(
            Duration::from_secs(10),
            send_bounded_with(&as_conn(&c), serde_json::json!({"chi": "chunk"}), &stats, short),
        )
        .await
        .expect("send_bounded must return, not hang");

        assert_eq!(got, Err(SendError::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(5), "returned promptly");
    }

    /// Bounding the send is pointless unless it also *removes* the peer:
    /// a connection left open keeps the lease calling a dead peer live.
    #[tokio::test]
    async fn a_stalled_peer_is_closed_on_timeout() {
        let c = conn(false);
        c.stalled.store(true, Ordering::SeqCst);
        let stats = SendStats::new();
        let _ = send_bounded_with(
            &as_conn(&c),
            serde_json::json!({"chi": "chunk"}),
            &stats,
            Duration::from_millis(50),
        )
        .await;
        assert!(c.closed.load(Ordering::SeqCst), "a stalled peer must be closed");
        assert_eq!(stats.timed_out(), 1);
    }

    /// The healthy path is untouched, and not counted.
    #[tokio::test]
    async fn a_healthy_peer_sends_and_is_not_counted() {
        let c = conn(false);
        let stats = SendStats::new();
        let got =
            send_bounded_with(&as_conn(&c), serde_json::json!({"chi": "chunk"}), &stats, SEND_TIMEOUT)
                .await;
        assert_eq!(got, Ok(()));
        assert_eq!(*c.sends.lock(), 1);
        assert_eq!(stats.timed_out(), 0);
        assert!(!c.closed.load(Ordering::SeqCst), "a working peer stays open");
    }

    /// A transport that errors is a different failure from one that
    /// hangs, and the two should be distinguishable in the counts.
    #[tokio::test]
    async fn a_failed_write_is_counted_separately_from_a_stall() {
        struct Broken(HumdAddr, PeerCapabilities);
        #[async_trait::async_trait]
        impl PeerConnection for Broken {
            fn peer(&self) -> &HumdAddr {
                &self.0
            }
            fn capabilities(&self) -> &PeerCapabilities {
                &self.1
            }
            async fn send(&self, _t: Tone) -> anyhow::Result<()> {
                anyhow::bail!("write failed")
            }
            fn take_receiver(&self) -> Option<tokio::sync::mpsc::Receiver<Tone>> {
                None
            }
            fn close(&self) {}
        }
        let c: Arc<dyn PeerConnection> =
            Arc::new(Broken(
                HumdAddr { id: Hid::random_humd(), hints: vec![] },
                PeerCapabilities::default(),
            ));
        let stats = SendStats::new();
        let got = send_bounded_with(&c, serde_json::json!({"chi": "chunk"}), &stats, SEND_TIMEOUT).await;
        assert!(
            matches!(got, Err(SendError::Failed(_))),
            "got {got:?}"
        );
        assert_eq!(stats.failed(), 1);
        assert_eq!(stats.timed_out(), 0, "a failure is not a stall");
    }
}

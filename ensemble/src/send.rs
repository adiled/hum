use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::task::JoinSet;

use crate::{PeerConnection, Tone};

pub const SEND_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SendError {
    #[error("peer stalled: no write completed within the send deadline")]
    TimedOut,
    #[error("write failed: {0}")]
    Failed(String),
}

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

/// Resolves when every send has landed or hit its deadline.
pub async fn fanout(
    sends: Vec<(Arc<dyn PeerConnection>, Tone)>,
    stats: &Arc<SendStats>,
) -> Vec<SendError> {
    let mut set = JoinSet::new();
    for (conn, tone) in sends {
        let stats = stats.clone();
        set.spawn(async move { send_bounded(&conn, tone, &stats).await });
    }
    let mut out = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Err(e)) => out.push(e),
            Ok(Ok(())) => {}
            Err(e) => tracing::error!(target: "ensemble.send", error = %e, "fanout.task.panicked"),
        }
    }
    out
}

pub async fn send_bounded(
    conn: &Arc<dyn PeerConnection>,
    tone: Tone,
    stats: &SendStats,
) -> Result<(), SendError> {
    send_bounded_with(conn, tone, stats, SEND_TIMEOUT).await
}

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

pub async fn send_opening_bounded(
    conn: &Arc<dyn PeerConnection>,
    tone: Tone,
    stats: &SendStats,
    deadline: Duration,
) -> Result<(), SendError> {
    // `send_opening`, not `send`: `send` parks on the gate this frame opens.
    match tokio::time::timeout(deadline, conn.send_opening(tone)).await {
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
                "opening.timed_out: peer never took the hello, closing"
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
            addr: HumdAddr {
                id: Hid::random_humd(),
                hints: vec![],
            },
            caps: PeerCapabilities::default(),
            stalled: AtomicBool::new(stalls),
            closed: AtomicBool::new(false),
            sends: Mutex::new(0),
        })
    }

    fn as_conn(c: &Arc<Stalled>) -> Arc<dyn PeerConnection> {
        c.clone()
    }

    struct Slow {
        addr: HumdAddr,
        caps: PeerCapabilities,
        delay: Duration,
        sent: AtomicU64,
    }

    #[async_trait::async_trait]
    impl PeerConnection for Slow {
        fn peer(&self) -> &HumdAddr {
            &self.addr
        }
        fn capabilities(&self) -> &PeerCapabilities {
            &self.caps
        }
        async fn send(&self, _tone: Tone) -> anyhow::Result<()> {
            tokio::time::sleep(self.delay).await;
            self.sent.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn take_receiver(&self) -> Option<tokio::sync::mpsc::Receiver<Tone>> {
            None
        }
        fn close(&self) {}
    }

    #[tokio::test]
    async fn a_stalled_opening_hits_its_deadline_and_closes_the_link() {
        let c = conn(false);
        c.stalled.store(true, Ordering::SeqCst);
        let stats = SendStats::new();

        let got = tokio::time::timeout(
            Duration::from_secs(10),
            send_opening_bounded(
                &as_conn(&c),
                serde_json::json!({"chi": "hello"}),
                &stats,
                Duration::from_millis(50),
            ),
        )
        .await
        .expect("a stalled opening must return, not hang");

        assert_eq!(got, Err(SendError::TimedOut));
        assert!(c.closed.load(Ordering::SeqCst), "a stalled opening must close the link");
        assert_eq!(stats.timed_out(), 1, "the timeout must be accounted for");
    }

    #[tokio::test]
    async fn a_fanout_costs_the_slowest_peer_not_the_sum() {
        let delay = Duration::from_millis(120);
        let peers = 5;
        let conns: Vec<Arc<Slow>> = (0..peers)
            .map(|_| {
                Arc::new(Slow {
                    addr: HumdAddr {
                        id: Hid::random_humd(),
                        hints: vec![],
                    },
                    caps: PeerCapabilities::default(),
                    delay,
                    sent: AtomicU64::new(0),
                })
            })
            .collect();

        let sends: Vec<(Arc<dyn PeerConnection>, Tone)> = conns
            .iter()
            .map(|c| {
                (
                    c.clone() as Arc<dyn PeerConnection>,
                    serde_json::json!({"chi": "chunk"}),
                )
            })
            .collect();

        let started = std::time::Instant::now();
        let errors = fanout(sends, &SendStats::new()).await;
        let elapsed = started.elapsed();

        assert!(errors.is_empty(), "healthy peers must not report errors");
        for c in &conns {
            assert_eq!(c.sent.load(Ordering::SeqCst), 1, "every peer got the tone");
        }
        assert!(
            elapsed < delay * 2,
            "a serial fanout would take {}ms; took {elapsed:?}",
            delay.as_millis() * peers
        );
    }

    #[tokio::test]
    async fn a_stalled_peer_times_out_rather_than_hanging() {
        let c = conn(false);
        c.stalled.store(true, Ordering::SeqCst);
        let stats = SendStats::new();
        let short = Duration::from_millis(50);

        let started = std::time::Instant::now();
        let got = tokio::time::timeout(
            Duration::from_secs(10),
            send_bounded_with(
                &as_conn(&c),
                serde_json::json!({"chi": "chunk"}),
                &stats,
                short,
            ),
        )
        .await
        .expect("send_bounded must return, not hang");

        assert_eq!(got, Err(SendError::TimedOut));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "returned promptly"
        );
    }

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
        assert!(
            c.closed.load(Ordering::SeqCst),
            "a stalled peer must be closed"
        );
        assert_eq!(stats.timed_out(), 1);
    }

    #[tokio::test]
    async fn a_healthy_peer_sends_and_is_not_counted() {
        let c = conn(false);
        let stats = SendStats::new();
        let got = send_bounded_with(
            &as_conn(&c),
            serde_json::json!({"chi": "chunk"}),
            &stats,
            SEND_TIMEOUT,
        )
        .await;
        assert_eq!(got, Ok(()));
        assert_eq!(*c.sends.lock(), 1);
        assert_eq!(stats.timed_out(), 0);
        assert!(
            !c.closed.load(Ordering::SeqCst),
            "a working peer stays open"
        );
    }

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
        let c: Arc<dyn PeerConnection> = Arc::new(Broken(
            HumdAddr {
                id: Hid::random_humd(),
                hints: vec![],
            },
            PeerCapabilities::default(),
        ));
        let stats = SendStats::new();
        let got = send_bounded_with(
            &c,
            serde_json::json!({"chi": "chunk"}),
            &stats,
            SEND_TIMEOUT,
        )
        .await;
        assert!(matches!(got, Err(SendError::Failed(_))), "got {got:?}");
        assert_eq!(stats.failed(), 1);
        assert_eq!(stats.timed_out(), 0, "a failure is not a stall");
    }
}

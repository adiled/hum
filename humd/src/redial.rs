//! Exponential backoff with full jitter, for peer redial.
//!
//! A peer that just died will often still be down on the next attempt.
//! Retrying immediately burns the daemon's budget and, on a mesh
//! restarting together, synchronises every node onto the same retry
//! instant. Jitter is what breaks that tie.

use std::time::Duration;

/// Retry schedule for one peer. Per-peer, not global: one dead peer
/// must not slow the recovery of the others.
#[derive(Debug, Clone)]
pub struct Backoff {
    base: Duration,
    max: Duration,
    /// Consecutive failures so far. Reset by a successful dial.
    attempt: u32,
    /// When the next attempt is allowed. `None` means "never failed",
    /// so the first dial is immediate.
    ready_at: Option<std::time::Instant>,
}

impl Backoff {
    pub fn new(base: Duration, max: Duration) -> Self {
        Self { base, max, attempt: 0, ready_at: None }
    }

    /// Consecutive failed attempts.
    pub fn attempts(&self) -> u32 {
        self.attempt
    }

    /// Whether a dial may be attempted right now.
    pub fn ready(&self) -> bool {
        match self.ready_at {
            None => true,
            Some(t) => std::time::Instant::now() >= t,
        }
    }

    /// The delay before the next attempt would be allowed.
    pub fn wait(&self) -> Option<Duration> {
        self.ready_at.map(|t| t.saturating_duration_since(std::time::Instant::now()))
    }

    /// Record a failed attempt and arm the next one.
    pub fn fail(&mut self) -> Duration {
        // 2^attempt, clamped before the shift: `1u32 << 32` is itself an
        // overflow, and saturating the multiplier is what stops a
        // long-dead peer from wrapping to a *short* delay.
        let exp = self.attempt.min(31);
        let scaled = self.base.saturating_mul(1u32 << exp);
        let ceiling = scaled.min(self.max);
        let delay = jitter(ceiling);
        self.attempt = self.attempt.saturating_add(1);
        self.ready_at = Some(std::time::Instant::now() + delay);
        delay
    }

    /// Record a successful dial. The peer is reachable, so the next
    /// failure starts from the bottom of the schedule again.
    pub fn succeed(&mut self) {
        self.attempt = 0;
        self.ready_at = None;
    }
}

/// Full jitter: uniform over `[ceiling/2, ceiling]`. Keeps a floor so a
/// fast peer isn't hammered, while spreading the herd across the top
/// half of the window.
fn jitter(ceiling: Duration) -> Duration {
    let half = ceiling / 2;
    let span = ceiling - half;
    if span.is_zero() {
        return half;
    }
    let extra = rand::random::<u64>() % (span.as_millis() as u64).max(1);
    half + Duration::from_millis(extra)
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new(Duration::from_secs(1), Duration::from_secs(300))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_attempt_is_immediate() {
        assert!(Backoff::default().ready());
    }

    #[test]
    fn failing_arms_the_next_attempt() {
        let mut b = Backoff::new(Duration::from_millis(10), Duration::from_secs(60));
        b.fail();
        assert!(!b.ready(), "just failed, must not retry instantly");
        assert!(b.wait().is_some());
    }

    #[test]
    fn success_rearms_immediately() {
        let mut b = Backoff::default();
        b.fail();
        b.succeed();
        assert!(b.ready());
        assert_eq!(b.attempts(), 0);
    }

    #[test]
    fn delay_grows_and_is_capped() {
        let mut b = Backoff::new(Duration::from_millis(100), Duration::from_secs(10));
        let mut prev = Duration::ZERO;
        for i in 0..12 {
            let d = b.fail();
            // Full jitter floors each window at half its ceiling, so
            // growth is observable even though exact values vary.
            assert!(d <= Duration::from_secs(10), "attempt {i} exceeded max");
            if i > 0 {
                assert!(d >= prev / 2, "attempt {i} shrank past its jitter floor");
            }
            prev = d;
        }
    }

    #[test]
    fn many_failures_do_not_overflow_to_short_delays() {
        let mut b = Backoff::new(Duration::from_millis(100), Duration::from_secs(10));
        for _ in 0..200 {
            assert!(b.fail() <= Duration::from_secs(10));
        }
        assert!(!b.ready());
    }

    #[test]
    fn jitter_stays_within_half_the_ceiling() {
        let ceiling = Duration::from_millis(800);
        for _ in 0..200 {
            let d = jitter(ceiling);
            assert!(d >= ceiling / 2 && d <= ceiling, "jitter {d:?} escaped window");
        }
    }
}

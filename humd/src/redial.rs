
use std::time::Duration;

const MAX_SHIFT: u32 = 31;

#[derive(Debug, Clone)]
pub struct Backoff {
    base: Duration,
    max: Duration,
    failures: u32,
    ready_at: Option<std::time::Instant>,
}

impl Backoff {
    pub fn new(base: Duration, max: Duration) -> Self {
        Self { base, max, failures: 0, ready_at: None }
    }

    pub fn failures(&self) -> u32 {
        self.failures
    }

    pub fn ready(&self) -> bool {
        match self.ready_at {
            None => true,
            Some(t) => std::time::Instant::now() >= t,
        }
    }

    pub fn wait(&self) -> Option<Duration> {
        self.ready_at.map(|t| t.saturating_duration_since(std::time::Instant::now()))
    }

    pub fn fail(&mut self) -> Duration {
        let scaled = self.base.saturating_mul(1u32 << self.failures.min(MAX_SHIFT));
        let ceiling = scaled.min(self.max);
        let delay = jitter(ceiling);
        self.failures = self.failures.saturating_add(1);
        self.ready_at = Some(std::time::Instant::now() + delay);
        delay
    }

    pub fn succeed(&mut self) {
        self.failures = 0;
        self.ready_at = None;
    }
}

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
        assert_eq!(b.failures(), 0);
    }

    #[test]
    fn delay_grows_and_is_capped() {
        let mut b = Backoff::new(Duration::from_millis(100), Duration::from_secs(10));
        let mut prev = Duration::ZERO;
        for i in 0..12 {
            let d = b.fail();
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

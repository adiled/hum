use governor::{Quota, RateLimiter};
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

#[tokio::test]
async fn rate_limiter_paces_accepts() {
    let limiter = RateLimiter::direct(Quota::per_second(NonZeroU32::new(10).unwrap()));
    let start = Instant::now();
    for _ in 0..20 {
        limiter.until_ready().await;
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(900),
        "20 acquisitions at 10/s should take ~1s, took {elapsed:?}",
    );
}

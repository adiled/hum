//! Per-roost observability — RSS, CPU, fd count, wall-clock age.
//!
//! A `RoostMetrics` is a point-in-time snapshot of one roost's OS-visible
//! resource use. `sample(pid, spawned_at_ms)` reads it. On Linux we lift
//! the numbers out of `/proc/<pid>/{statm,stat,fd}`; on other platforms
//! we fill in just the fields we can compute without the kernel (age,
//! sampled_at) and leave the rest as `None` — the caller logs that the
//! platform doesn't support full sampling.
//!
//! Sampling is best-effort. A process that exits between the `read_dir`
//! and the `read_to_string` shows up as a partial snapshot rather than
//! a panic. That's deliberate: roosts come and go, and a metrics layer
//! that crashes its sampler defeats the purpose.
//!
//! The optional `spawn_sampler` helper runs a sampler in a tokio task at
//! a fixed interval and pushes snapshots into an unbounded channel. The
//! daemon owns the receiver and decides whether to aggregate / log /
//! forward upstream.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Point-in-time snapshot of one roost's OS-visible resource use.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RoostMetrics {
    pub pid: Option<u32>,
    /// Resident set size in bytes. None if unavailable on this platform.
    pub rss_bytes: Option<u64>,
    /// Cumulative CPU time in milliseconds (user+system).
    pub cpu_ms: Option<u64>,
    /// Open file descriptor count. None if unavailable.
    pub fd_count: Option<u32>,
    /// Wall-clock age of the roost in milliseconds.
    pub age_ms: u64,
    /// Wall-clock timestamp this snapshot was taken (ms since UNIX epoch).
    pub sampled_at_ms: i64,
}

/// Read a one-shot snapshot for the given pid. On Linux this reads from
/// /proc/<pid>/{statm, stat, fd}. On other platforms it returns a
/// snapshot with most fields set to None; this is deliberate — the
/// caller logs that the platform doesn't support full sampling.
pub fn sample(pid: u32, spawned_at_ms: i64) -> RoostMetrics {
    let sampled_at_ms = now_ms();
    let age_ms = sampled_at_ms
        .saturating_sub(spawned_at_ms)
        .max(0) as u64;

    let mut m = RoostMetrics {
        pid: Some(pid),
        rss_bytes: None,
        cpu_ms: None,
        fd_count: None,
        age_ms,
        sampled_at_ms,
    };

    #[cfg(target_os = "linux")]
    {
        m.rss_bytes = read_rss_linux(pid);
        m.cpu_ms = read_cpu_ms_linux(pid);
        m.fd_count = read_fd_count_linux(pid);
    }

    m
}

/// Spawn a background tokio task that samples the given pid every
/// `interval` and pushes snapshots into the returned receiver. The task
/// exits when the receiver is dropped (channel send fails).
///
/// The returned `JoinHandle` lets the caller abort the sampler early.
pub fn spawn_sampler(
    pid: u32,
    spawned_at_ms: i64,
    interval: std::time::Duration,
) -> (mpsc::UnboundedReceiver<RoostMetrics>, tokio::task::JoinHandle<()>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(async move {
        let mut tick = tokio::time::interval(interval);
        // First tick fires immediately; we want one sample up front.
        loop {
            tick.tick().await;
            let snap = sample(pid, spawned_at_ms);
            if tx.send(snap).is_err() {
                // Receiver dropped — no one is listening, so stop sampling.
                break;
            }
        }
    });
    (rx, handle)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---- Linux /proc parsing ----------------------------------------------------

#[cfg(target_os = "linux")]
fn page_size() -> u64 {
    // SAFETY: sysconf is thread-safe and always defined for _SC_PAGESIZE.
    let p = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if p > 0 {
        p as u64
    } else {
        // Sane fallback for x86_64/aarch64. Almost never hit.
        4096
    }
}

#[cfg(target_os = "linux")]
fn clk_tck() -> u64 {
    let t = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if t > 0 {
        t as u64
    } else {
        100
    }
}

/// /proc/<pid>/statm: "size resident shared text lib data dt" in pages.
/// We want word index 1 (resident) * page_size.
#[cfg(target_os = "linux")]
fn read_rss_linux(pid: u32) -> Option<u64> {
    let raw = std::fs::read_to_string(format!("/proc/{}/statm", pid)).ok()?;
    let mut it = raw.split_ascii_whitespace();
    let _size = it.next()?;
    let resident: u64 = it.next()?.parse().ok()?;
    Some(resident.saturating_mul(page_size()))
}

/// /proc/<pid>/stat: utime is field 14, stime is field 15 (1-indexed).
/// Field 2 is `(comm)` which may contain spaces and parens — locate the
/// LAST `)` and split from there so the fields after `comm` are safe to
/// whitespace-split.
#[cfg(target_os = "linux")]
fn read_cpu_ms_linux(pid: u32) -> Option<u64> {
    let raw = std::fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    let close = raw.rfind(')')?;
    // After the ')' there's a space then field 3 (state). So field N (N>=3)
    // in the original 1-indexed layout is at index (N - 3) of the tail
    // split by whitespace.
    let tail = raw[close + 1..].trim();
    let parts: Vec<&str> = tail.split_ascii_whitespace().collect();
    // utime = field 14 → tail index 14 - 3 = 11
    // stime = field 15 → tail index 15 - 3 = 12
    let utime: u64 = parts.get(11)?.parse().ok()?;
    let stime: u64 = parts.get(12)?.parse().ok()?;
    let ticks = utime.saturating_add(stime);
    let hz = clk_tck();
    if hz == 0 {
        return None;
    }
    Some(ticks.saturating_mul(1000) / hz)
}

/// Count entries in /proc/<pid>/fd. Each entry is one open fd.
#[cfg(target_os = "linux")]
fn read_fd_count_linux(pid: u32) -> Option<u32> {
    let dir = std::fs::read_dir(format!("/proc/{}/fd", pid)).ok()?;
    let mut n: u32 = 0;
    for e in dir {
        // Ignore individual errors — fds churn while we iterate.
        if e.is_ok() {
            n = n.saturating_add(1);
        }
    }
    Some(n)
}

// ---- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn now() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sample_self_has_rss_on_linux() {
        let snap = sample(std::process::id(), now());
        assert!(snap.rss_bytes.is_some(), "rss missing for self pid");
        assert!(snap.rss_bytes.unwrap() > 0, "rss should be > 0");
        // While we're here: cpu_ms and fd_count should also resolve for self.
        assert!(snap.cpu_ms.is_some(), "cpu_ms missing for self pid");
        assert!(snap.fd_count.is_some(), "fd_count missing for self pid");
    }

    #[test]
    fn sample_unknown_pid_returns_defaults() {
        let snap = sample(999_999_999, now());
        assert!(snap.rss_bytes.is_none(), "no rss for nonexistent pid");
        assert!(snap.cpu_ms.is_none(), "no cpu_ms for nonexistent pid");
        assert!(snap.fd_count.is_none(), "no fd_count for nonexistent pid");
        assert_eq!(snap.pid, Some(999_999_999));
    }

    #[test]
    fn age_ms_increases_over_time() {
        let spawned = now();
        let a = sample(std::process::id(), spawned);
        std::thread::sleep(Duration::from_millis(10));
        let b = sample(std::process::id(), spawned);
        assert!(
            b.age_ms > a.age_ms,
            "age_ms should grow: a={} b={}",
            a.age_ms,
            b.age_ms
        );
    }
}

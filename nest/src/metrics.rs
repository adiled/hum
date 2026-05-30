//! Per-cell observability — RSS, CPU, fd count, wall-clock age.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CellMetrics {
    pub pid: Option<u32>,
    pub rss_bytes: Option<u64>,
    pub cpu_ms: Option<u64>,
    pub fd_count: Option<u32>,
    pub age_ms: u64,
    pub sampled_at_ms: i64,
}

pub fn sample(pid: u32, spawned_at_ms: i64) -> CellMetrics {
    let sampled_at_ms = now_ms();
    let age_ms = sampled_at_ms.saturating_sub(spawned_at_ms).max(0) as u64;
    let mut m = CellMetrics {
        pid: Some(pid),
        age_ms,
        sampled_at_ms,
        ..Default::default()
    };

    let mut sys = System::new_with_specifics(
        RefreshKind::new().with_processes(ProcessRefreshKind::everything()),
    );
    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), true);
    if let Some(p) = sys.process(Pid::from_u32(pid)) {
        m.rss_bytes = Some(p.memory());
        m.cpu_ms = Some(p.cpu_usage() as u64);
    }

    #[cfg(target_os = "linux")]
    {
        m.fd_count = fd_count_linux(pid);
    }

    m
}

pub fn spawn_sampler(
    pid: u32,
    spawned_at_ms: i64,
    interval: std::time::Duration,
) -> (mpsc::UnboundedReceiver<CellMetrics>, tokio::task::JoinHandle<()>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(async move {
        let mut tick = tokio::time::interval(interval);
        loop {
            tick.tick().await;
            let snap = sample(pid, spawned_at_ms);
            if tx.send(snap).is_err() { break; }
        }
    });
    (rx, handle)
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[cfg(target_os = "linux")]
fn fd_count_linux(pid: u32) -> Option<u32> {
    std::fs::read_dir(format!("/proc/{}/fd", pid)).ok()
        .map(|d| d.flatten().count() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn now() -> i64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
    }

    #[test]
    fn sample_self_has_rss() {
        let snap = sample(std::process::id(), now());
        assert!(snap.rss_bytes.unwrap_or(0) > 0, "rss should be non-zero for self");
    }

    #[test]
    fn sample_unknown_pid_returns_defaults() {
        let snap = sample(999_999_999, now());
        assert!(snap.rss_bytes.is_none());
        assert!(snap.cpu_ms.is_none());
    }

    #[tokio::test]
    async fn sampler_emits_snapshots() {
        let (mut rx, handle) = spawn_sampler(std::process::id(), now(), Duration::from_millis(50));
        let first = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await
            .expect("no sample within 1s").expect("channel closed");
        assert_eq!(first.pid, Some(std::process::id()));
        handle.abort();
    }
}

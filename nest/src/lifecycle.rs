//! Own a child process group; expose a cancel handle that tree-kills.

use std::time::Duration;

use command_group::AsyncGroupChild;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tracing::warn;

const REAP_TIMEOUT: Duration = Duration::from_secs(5);
const SIGKILL_EXIT: i32 = 137;

pub fn supervise(mut child: AsyncGroupChild) -> (oneshot::Receiver<i32>, CancellationToken) {
    let cancel = CancellationToken::new();
    let cancel_for_task = cancel.clone();
    let (tx_exit, rx_exit) = oneshot::channel();
    let pid = child.inner().id();

    tokio::spawn(async move {
        let code = tokio::select! {
            biased;
            _ = cancel_for_task.cancelled() => kill_and_reap(&mut child, pid).await,
            result = child.wait() => match result {
                Ok(status) => status.code().unwrap_or(1),
                Err(e) => { warn!(target: "nest::lifecycle", pid = ?pid, err = %e, "cell.wait_failed"); 1 }
            }
        };
        let _ = tx_exit.send(code);
    });

    (rx_exit, cancel)
}

async fn kill_and_reap(child: &mut AsyncGroupChild, pid: Option<u32>) -> i32 {
    metrics::counter!("hum_cell_kills_total").increment(1);
    if let Err(e) = child.kill().await {
        warn!(target: "nest::lifecycle", pid = ?pid, err = %e, "cell.kill.signal_failed");
    }
    match tokio::time::timeout(REAP_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => status.code().unwrap_or(SIGKILL_EXIT),
        Ok(Err(e)) => { warn!(target: "nest::lifecycle", pid = ?pid, err = %e, "cell.kill.reap_failed"); SIGKILL_EXIT }
        Err(_)     => { warn!(target: "nest::lifecycle", pid = ?pid, "cell.kill.reap_timeout"); SIGKILL_EXIT }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use command_group::AsyncCommandGroup;
    use std::process::Stdio;
    use std::time::Instant;
    use tokio::process::Command;

    #[tokio::test]
    async fn cancel_kills_the_child() {
        let child = Command::new("sleep").arg("60")
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .group_spawn().expect("spawn sleep");
        let (rx_exit, cancel) = supervise(child);
        let start = Instant::now();
        cancel.cancel();
        let code = tokio::time::timeout(Duration::from_secs(REAP_TIMEOUT.as_secs() + 1), rx_exit)
            .await.expect("supervisor stuck").expect("exit channel dropped");
        assert!(start.elapsed() < REAP_TIMEOUT);
        assert_eq!(code, SIGKILL_EXIT);
    }

    #[tokio::test]
    async fn natural_exit_propagates_code() {
        let child = Command::new("sh").arg("-c").arg("exit 42")
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .group_spawn().expect("spawn sh");
        let (rx_exit, _cancel) = supervise(child);
        let code = tokio::time::timeout(Duration::from_secs(5), rx_exit)
            .await.expect("stuck").expect("dropped");
        assert_eq!(code, 42);
    }

    #[tokio::test]
    async fn tree_kill_takes_descendants() {
        let marker = std::env::temp_dir().join(format!("hum-tree-kill-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let marker_path = marker.to_string_lossy().to_string();
        let script = format!(
            "sh -c 'sleep 60 & echo $! > {marker_path}; wait'"
        );
        let child = Command::new("sh").arg("-c").arg(&script)
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .group_spawn().expect("spawn sh");
        let (_rx, cancel) = supervise(child);

        // wait for the grandchild pid to appear
        for _ in 0..30 {
            if marker.exists() { break; }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let grandchild_pid: u32 = std::fs::read_to_string(&marker)
            .expect("grandchild marker").trim().parse().expect("pid parse");

        cancel.cancel();
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Sending signal 0 returns ESRCH if the process is gone.
        let alive = unsafe { libc::kill(grandchild_pid as i32, 0) } == 0;
        let _ = std::fs::remove_file(&marker);
        assert!(!alive, "grandchild {grandchild_pid} survived tree-kill");
    }
}

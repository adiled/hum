use std::path::PathBuf;

use tokio::fs::OpenOptions;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdout, ChildStderr};
use tracing::warn;

fn logs_dir() -> PathBuf {
    hum_paths::state_dir().join("logs")
}

pub fn out_log_path(kind: &str) -> PathBuf {
    logs_dir().join(format!("{kind}.out.log"))
}

pub fn err_log_path(kind: &str) -> PathBuf {
    logs_dir().join(format!("{kind}.err.log"))
}

/// Tail child stdout into the per-bee out log file. Spawns a task that
/// runs until the stream ends.
pub fn pipe_stdout(kind: String, stream: ChildStdout) {
    let path = out_log_path(&kind);
    tokio::spawn(async move {
        if let Err(e) = pipe(stream, path).await {
            warn!(%kind, err = %e, "humnest.log.stdout_failed");
        }
    });
}

pub fn pipe_stderr(kind: String, stream: ChildStderr) {
    let path = err_log_path(&kind);
    tokio::spawn(async move {
        if let Err(e) = pipe(stream, path).await {
            warn!(%kind, err = %e, "humnest.log.stderr_failed");
        }
    });
}

async fn pipe<R: tokio::io::AsyncRead + Unpin>(stream: R, path: PathBuf) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(&path).await?;
    let mut lines = BufReader::new(stream).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let buf = format!("{line}\n");
        if file.write_all(buf.as_bytes()).await.is_err() { break; }
    }
    file.flush().await.ok();
    Ok(())
}

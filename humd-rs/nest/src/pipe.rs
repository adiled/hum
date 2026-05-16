//! PipePerch — `claude -p --input-format stream-json --output-format stream-json`.
//!
//! Real, fully working. Spawns claude via tokio::process with piped stdio,
//! reads stdout line-by-line as JSON, fans bytes from a stdin channel into
//! the child's stdin. stderr is logged via `tracing`.

use std::process::Stdio;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{trace, warn};

use crate::{Perch, PerchSpawnArgs, Roost};

pub struct PipePerch;

impl Default for PipePerch {
    fn default() -> Self {
        Self
    }
}

#[async_trait]
impl Perch for PipePerch {
    fn ephemeral(&self) -> bool {
        false
    }

    async fn spawn(&self, args: PerchSpawnArgs) -> Result<Roost> {
        let mut cmd = Command::new(&args.command);
        cmd.args(&args.args)
            .current_dir(&args.cwd)
            .env_clear()
            .envs(&args.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn().context("spawn claude (pipe)")?;
        let pid = child.id();
        let mut stdin = child.stdin.take().context("missing stdin")?;
        let stdout = child.stdout.take().context("missing stdout")?;
        let stderr = child.stderr.take().context("missing stderr")?;

        let (tx_in, mut rx_in) = mpsc::channel::<String>(64);
        let (tx_evt, rx_evt) = mpsc::channel::<Value>(256);
        let (tx_exit, rx_exit) = oneshot::channel::<i32>();

        // stdin pump
        tokio::spawn(async move {
            while let Some(line) = rx_in.recv().await {
                let mut buf = line.into_bytes();
                buf.push(b'\n');
                if let Err(e) = stdin.write_all(&buf).await {
                    warn!(target: "nest", "pipe.stdin.write.failed: {e}");
                    break;
                }
                if let Err(e) = stdin.flush().await {
                    warn!(target: "nest", "pipe.stdin.flush.failed: {e}");
                    break;
                }
            }
            trace!(target: "nest", "pipe.stdin.closed");
        });

        // stdout reader — parse line-delimited JSON, push Values to events
        let tx_evt_out = tx_evt.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            loop {
                match reader.next_line().await {
                    Ok(Some(line)) => {
                        let line = line.trim();
                        if line.is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<Value>(line) {
                            Ok(v) => {
                                if tx_evt_out.send(v).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                warn!(target: "nest", "pipe.stdout.parse.failed: {e}, line={}", &line.chars().take(200).collect::<String>());
                            }
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        warn!(target: "nest", "pipe.stdout.read.failed: {e}");
                        break;
                    }
                }
            }
            trace!(target: "nest", "pipe.stdout.eof");
        });

        // stderr → tracing
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let line = line.trim();
                if !line.is_empty() {
                    trace!(target: "nest", "pipe.stderr: {line}");
                }
            }
        });

        // exit watcher — owns the Child so it stays alive
        let kill_arc: std::sync::Arc<dyn Fn() + Send + Sync> = {
            // The Child's kill() requires &mut self. Stash it behind an
            // async-safe mutex shared between the exit watcher and the kill
            // callback. The watcher holds the mutex only when waiting/exiting.
            let child_holder = std::sync::Arc::new(Mutex::new(Some(child)));
            let holder_for_wait = child_holder.clone();
            tokio::spawn(async move {
                let code = {
                    let mut guard = holder_for_wait.lock().await;
                    match guard.as_mut() {
                        Some(c) => c.wait().await.map(|s| s.code().unwrap_or(1)).unwrap_or(1),
                        None => 1,
                    }
                };
                let _ = tx_exit.send(code);
            });
            let holder_for_kill = child_holder.clone();
            std::sync::Arc::new(move || {
                // Try to kill without blocking. start_kill() is sync.
                if let Ok(mut guard) = holder_for_kill.try_lock() {
                    if let Some(c) = guard.as_mut() {
                        let _ = c.start_kill();
                    }
                }
            })
        };

        trace!(target: "nest", "pipe.spawned pid={:?}", pid);

        Ok(Roost {
            pid,
            stdin: tx_in,
            events: std::sync::Arc::new(Mutex::new(rx_evt)),
            exited: rx_exit,
            ephemeral: false,
            kill: kill_arc,
        })
    }
}

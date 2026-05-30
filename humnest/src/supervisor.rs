use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use command_group::AsyncCommandGroup;
use parking_lot::RwLock;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use serde::Serialize;

use config::BeeConfig;
use nest::lifecycle;

#[derive(Debug, Clone)]
pub enum RestartPolicy {
    Always,
    OnFailure { max_retries: u32, backoff_ms: u64 },
    Never,
}

impl RestartPolicy {
    fn from_config(c: &BeeConfig) -> Self {
        match c.restart.as_str() {
            "always" => RestartPolicy::Always,
            "on-failure" => RestartPolicy::OnFailure { max_retries: c.max_retries, backoff_ms: c.backoff_ms },
            "never" => RestartPolicy::Never,
            _ => RestartPolicy::Always,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BeeStatus {
    pub kind: String,
    pub pid: Option<u32>,
    pub state: String,           // "running" | "exited" | "crash-loop" | "stopped"
    pub restart_count: u32,
    pub last_exit_code: Option<i32>,
}

struct BeeSlot {
    kind: String,
    cancel: CancellationToken,
    pid: Option<u32>,
    state: String,
    restart_count: u32,
    last_exit_code: Option<i32>,
}

pub struct Supervisor {
    slots: RwLock<HashMap<String, Arc<RwLock<BeeSlot>>>>,
}

impl Supervisor {
    pub fn new() -> Self {
        Self { slots: RwLock::new(HashMap::new()) }
    }

    pub async fn spawn_all(self: Arc<Self>, bees: Vec<BeeConfig>) {
        for b in bees {
            if let Err(e) = self.clone().spawn_one(b).await {
                warn!(err = %e, "humnest.spawn.failed");
            }
        }
    }

    pub async fn spawn_one(self: Arc<Self>, cfg: BeeConfig) -> Result<()> {
        let kind = cfg.kind.clone();
        let policy = RestartPolicy::from_config(&cfg);
        let cancel = CancellationToken::new();
        let slot = Arc::new(RwLock::new(BeeSlot {
            kind: kind.clone(),
            cancel: cancel.clone(),
            pid: None,
            state: "spawning".into(),
            restart_count: 0,
            last_exit_code: None,
        }));
        self.slots.write().insert(kind.clone(), slot.clone());

        let this = self.clone();
        tokio::spawn(async move {
            let mut retries = 0u32;
            loop {
                let argv = if cfg.argv.is_empty() {
                    vec![format!("hum-{}-worker", cfg.kind)]
                } else { cfg.argv.clone() };

                let mut cmd = Command::new(&argv[0]);
                cmd.args(&argv[1..]);
                for (k, v) in &cfg.env { cmd.env(k, v); }

                let mut child = match cmd.group_spawn() {
                    Ok(c) => c,
                    Err(e) => {
                        warn!(kind = %cfg.kind, err = %e, "humnest.bee.spawn_failed");
                        slot.write().state = "exited".into();
                        return;
                    }
                };
                let pid = child.inner().id();
                slot.write().pid = pid;
                slot.write().state = "running".into();
                info!(kind = %cfg.kind, pid = ?pid, "humnest.bee.spawned");

                let (rx_exit, child_cancel) = lifecycle::supervise(child);

                tokio::select! {
                    code = rx_exit => {
                        let code = code.unwrap_or(1);
                        slot.write().last_exit_code = Some(code);
                        slot.write().pid = None;
                        info!(kind = %cfg.kind, code, "humnest.bee.exited");

                        if cancel.is_cancelled() {
                            slot.write().state = "stopped".into();
                            return;
                        }

                        match &policy {
                            RestartPolicy::Never => {
                                slot.write().state = "exited".into();
                                return;
                            }
                            RestartPolicy::OnFailure { max_retries, .. } if code == 0 => {
                                slot.write().state = "exited".into();
                                return;
                            }
                            RestartPolicy::OnFailure { max_retries, backoff_ms } => {
                                retries += 1;
                                if retries > *max_retries {
                                    slot.write().state = "crash-loop".into();
                                    warn!(kind = %cfg.kind, retries, "humnest.bee.crash_loop");
                                    return;
                                }
                                slot.write().restart_count = retries;
                                tokio::time::sleep(Duration::from_millis(*backoff_ms)).await;
                            }
                            RestartPolicy::Always => {
                                retries += 1;
                                slot.write().restart_count = retries;
                                tokio::time::sleep(Duration::from_millis(cfg.backoff_ms)).await;
                            }
                        }
                    }
                    _ = cancel.cancelled() => {
                        child_cancel.cancel();
                        slot.write().state = "stopped".into();
                        info!(kind = %cfg.kind, "humnest.bee.stopping");
                        return;
                    }
                }
            }
        });
        let _ = this;
        Ok(())
    }

    pub async fn kill_one(&self, kind: &str) -> Result<()> {
        if let Some(slot) = self.slots.read().get(kind).cloned() {
            slot.read().cancel.cancel();
            Ok(())
        } else {
            anyhow::bail!("no such bee: {kind}");
        }
    }

    pub async fn kill_all(&self) {
        let cancels: Vec<_> = self.slots.read().values().map(|s| s.read().cancel.clone()).collect();
        for c in cancels { c.cancel(); }
    }

    pub fn list(&self) -> Vec<BeeStatus> {
        self.slots.read().values().map(|s| {
            let s = s.read();
            BeeStatus {
                kind: s.kind.clone(),
                pid: s.pid,
                state: s.state.clone(),
                restart_count: s.restart_count,
                last_exit_code: s.last_exit_code,
            }
        }).collect()
    }
}

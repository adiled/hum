//! claude-cli — the pipe-mode nestling for claude.
//!
//! `claude -p --input-format stream-json --output-format stream-json`.
//! Takes a [`nest::SpawnSpec`], builds the CLI invocation, runs the
//! subprocess, exposes stdin/stdout/exit through [`nest::Roost`]. The
//! daemon never sees claude-specific arg shapes — this crate owns them.

pub mod graft;

use std::process::Stdio;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{trace, warn};

use nest::{Perch, Propensity, Roost, SpawnSpec};

pub struct ClaudeCliPerch;

impl Default for ClaudeCliPerch {
    fn default() -> Self { Self }
}

/// Build the claude CLI argv from a [`SpawnSpec`].
///
/// Public so it can be unit-tested without spawning a real process. Pure
/// function — no IO.
pub fn build_argv(spec: &SpawnSpec) -> Vec<String> {
    let mut argv = vec![
        "-p".to_string(),
        "--verbose".to_string(),
        "--model".to_string(), spec.model_id.clone(),
        "--dangerously-skip-permissions".to_string(),
        "--disable-slash-commands".to_string(),
        "--input-format".to_string(), "stream-json".to_string(),
        "--output-format".to_string(), "stream-json".to_string(),
        "--include-partial-messages".to_string(),
    ];
    if let Some(mcp_url) = spec.mcp_url.as_deref() {
        let mcp_config = serde_json::json!({
            "mcpServers": {
                "hum": { "type": "http", "url": format!("{}/s/{}", mcp_url, spec.sid) }
            }
        }).to_string();
        argv.push("--mcp-config".into());
        argv.push(mcp_config);
        argv.push("--strict-mcp-config".into());
    }
    // Pure pass-through. Perch invents no policy; humd populates these
    // from the nestler's hello (opt-in). Empty vec = no flag.
    if !spec.allowed_tools.is_empty() {
        argv.push("--allowed-tools".into());
        argv.push(spec.allowed_tools.join(" "));
    }
    if !spec.disallowed_tools.is_empty() {
        argv.push("--disallowed-tools".into());
        argv.push(spec.disallowed_tools.join(" "));
    }
    if let Some(sp) = spec.system_prompt.as_deref() {
        argv.push("--system-prompt".into());
        argv.push(sp.to_string());
    }
    if let Some(resume) = spec.resume_id.as_deref() {
        argv.push("--resume".into());
        argv.push(resume.to_string());
    }
    argv
}

/// Build the spawn env. claude is sensitive to a small set of toggles;
/// callers can override anything via `spec.env`.
pub fn build_env(spec: &SpawnSpec) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = vec![
        ("CLAUDE_CODE_DISABLE_CLAUDE_MDS".into(), "1".into()),
        ("CLAUDE_CODE_DISABLE_AUTO_MEMORY".into(), "1".into()),
        ("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS".into(), "1".into()),
        ("CLAUDE_CODE_DISABLE_FAST_MODE".into(), "1".into()),
        ("DISABLE_INTERLEAVED_THINKING".into(), "1".into()),
        ("ENABLE_TOOL_SEARCH".into(), "false".into()),
    ];
    if !spec.plan_mode {
        env.push(("CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING".into(), "1".into()));
    }
    // Inherit a minimal set so claude can find its siblings + user dirs.
    if let Ok(path) = std::env::var("PATH") {
        env.push(("PATH".into(), path));
    }
    if let Ok(home) = std::env::var("HOME") {
        env.push(("HOME".into(), home));
    }
    for (k, v) in &spec.env {
        env.push((k.clone(), v.clone()));
    }
    env
}

#[async_trait]
impl Perch for ClaudeCliPerch {
    fn ephemeral(&self) -> bool { false }
    fn propensity(&self) -> Propensity { Propensity::StatefulSession }

    async fn spawn(&self, spec: SpawnSpec) -> Result<Roost> {
        let cli = spec.cli_path.clone()
            .or_else(|| std::env::var("CLAUDE_CLI_PATH").ok())
            .unwrap_or_else(|| "claude".into());
        let argv = build_argv(&spec);
        let env = build_env(&spec);

        let mut cmd = Command::new(&cli);
        cmd.args(&argv)
            .current_dir(&spec.cwd)
            .env_clear()
            .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn().with_context(|| format!("spawn {cli}"))?;
        let pid = child.id();
        let mut stdin = child.stdin.take().context("missing stdin")?;
        let stdout = child.stdout.take().context("missing stdout")?;
        let stderr = child.stderr.take().context("missing stderr")?;

        let (tx_in, mut rx_in) = mpsc::channel::<String>(64);
        let (tx_evt, rx_evt) = mpsc::channel::<Value>(256);
        let (tx_exit, rx_exit) = oneshot::channel::<i32>();

        // stdin pump — append `\n` for NDJSON framing.
        tokio::spawn(async move {
            while let Some(line) = rx_in.recv().await {
                let mut buf = line.into_bytes();
                buf.push(b'\n');
                if let Err(e) = stdin.write_all(&buf).await {
                    warn!(target: "claude-cli", "stdin.write.failed: {e}");
                    break;
                }
                if let Err(e) = stdin.flush().await {
                    warn!(target: "claude-cli", "stdin.flush.failed: {e}");
                    break;
                }
            }
            trace!(target: "claude-cli", "stdin.closed");
        });

        // stdout reader — parse line-delimited JSON, push Values to events.
        let tx_evt_out = tx_evt.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            loop {
                match reader.next_line().await {
                    Ok(Some(line)) => {
                        let line = line.trim();
                        if line.is_empty() { continue; }
                        match serde_json::from_str::<Value>(line) {
                            Ok(v) => {
                                if tx_evt_out.send(v).await.is_err() { break; }
                            }
                            Err(e) => {
                                warn!(target: "claude-cli", "stdout.parse.failed: {e}, line={}", &line.chars().take(200).collect::<String>());
                            }
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        warn!(target: "claude-cli", "stdout.read.failed: {e}");
                        break;
                    }
                }
            }
            trace!(target: "claude-cli", "stdout.eof");
        });

        // stderr → tracing
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let line = line.trim();
                if !line.is_empty() {
                    trace!(target: "claude-cli", "stderr: {line}");
                }
            }
        });

        // exit watcher — keep Child alive behind an async-safe mutex so
        // both the wait task and the kill closure can reach it.
        let kill_arc: std::sync::Arc<dyn Fn() + Send + Sync> = {
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
                if let Ok(mut guard) = holder_for_kill.try_lock() {
                    if let Some(c) = guard.as_mut() {
                        let _ = c.start_kill();
                    }
                }
            })
        };

        trace!(target: "claude-cli", "spawned pid={:?}", pid);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_includes_basics() {
        let spec = SpawnSpec::new("sid-1", "claude-haiku-4-5", "/tmp");
        let argv = build_argv(&spec);
        assert!(argv.contains(&"-p".to_string()));
        assert!(argv.contains(&"--verbose".to_string()));
        assert!(argv.contains(&"claude-haiku-4-5".to_string()));
        assert!(argv.contains(&"--dangerously-skip-permissions".to_string()));
        assert!(argv.iter().any(|a| a == "stream-json"));
    }

    #[test]
    fn argv_omits_mcp_when_no_url() {
        let spec = SpawnSpec::new("s", "m", "/");
        let argv = build_argv(&spec);
        assert!(!argv.iter().any(|a| a == "--mcp-config"));
        assert!(!argv.iter().any(|a| a == "--strict-mcp-config"));
    }

    #[test]
    fn argv_includes_mcp_when_url_set() {
        let mut spec = SpawnSpec::new("sid-9", "m", "/");
        spec.mcp_url = Some("http://127.0.0.1:29147".into());
        let argv = build_argv(&spec);
        let idx = argv.iter().position(|a| a == "--mcp-config").expect("mcp-config flag");
        let config: serde_json::Value = serde_json::from_str(&argv[idx + 1]).unwrap();
        assert_eq!(
            config["mcpServers"]["hum"]["url"],
            "http://127.0.0.1:29147/s/sid-9"
        );
        assert!(argv.iter().any(|a| a == "--strict-mcp-config"));
    }

    #[test]
    fn argv_includes_system_prompt() {
        let mut spec = SpawnSpec::new("s", "m", "/");
        spec.system_prompt = Some("Be terse.".into());
        let argv = build_argv(&spec);
        let i = argv.iter().position(|a| a == "--system-prompt").unwrap();
        assert_eq!(argv[i + 1], "Be terse.");
    }

    #[test]
    fn argv_includes_resume() {
        let mut spec = SpawnSpec::new("s", "m", "/");
        spec.resume_id = Some("abc-123".into());
        let argv = build_argv(&spec);
        let i = argv.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(argv[i + 1], "abc-123");
    }

    #[test]
    fn env_disables_adaptive_thinking_when_not_planning() {
        let spec = SpawnSpec::new("s", "m", "/");
        let env = build_env(&spec);
        assert!(env.iter().any(|(k, v)| k == "CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING" && v == "1"));
    }

    #[test]
    fn env_keeps_adaptive_thinking_in_plan_mode() {
        let mut spec = SpawnSpec::new("s", "m", "/");
        spec.plan_mode = true;
        let env = build_env(&spec);
        assert!(!env.iter().any(|(k, _)| k == "CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING"));
    }

    #[test]
    fn env_user_override_wins() {
        let mut spec = SpawnSpec::new("s", "m", "/");
        spec.env.insert("CLAUDE_CODE_DISABLE_FAST_MODE".into(), "0".into());
        let env = build_env(&spec);
        let positions: Vec<(usize, &str)> = env.iter().enumerate()
            .filter(|(_, (k, _))| k == "CLAUDE_CODE_DISABLE_FAST_MODE")
            .map(|(i, (_, v))| (i, v.as_str()))
            .collect();
        assert!(positions.len() >= 2);
        assert_eq!(positions.last().unwrap().1, "0");
    }
}

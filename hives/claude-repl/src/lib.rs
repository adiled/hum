//! claude-repl — interactive `claude` over a PTY.
//!
//! Spawns the PTY, watches stdout, marks PERCHED when the prompt glyph
//! `❯` shows up. No transcript synth, no hooks, no classifier yet — the
//! cell compiles and runs but can't carry a bloom.
//!
//! As in claude-cli, a cell never works the host it runs on: Claude
//! Code's own host tools, [`HOST_TOOLS`], are always off, and its shell
//! is the bloom's `bash` over the nest's MCP. It is held to its nest's
//! bounds too, laid on it as it spawns, since a PTY spawn takes no
//! `pre_exec`.
//!
//! The cell is fed the sigil's tones and types a prompt's words; it gives
//! back a `cell-ready` pulse once the REPL is perched.

use std::io::Read;

use anyhow::{Context, Result};
use async_trait::async_trait;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{trace, warn};

use hum_nest::{Cell, Egg, Nurse};

pub const HOST_TOOLS: &[&str] = &[
    "Bash", "BashOutput", "KillShell", "KillBash",
    "Read", "Write", "Edit", "MultiEdit", "NotebookEdit",
    "Glob", "Grep", "LS",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HarnessState {
    Nesting,
    Perched,
    Felled,
}

pub struct ClaudeReplNurse;

impl Default for ClaudeReplNurse {
    fn default() -> Self {
        Self
    }
}

/// Build the claude args for REPL/PTY mode. No `-p`, no `--input-format` —
/// interactive Ink TUI mode. Pure function for unit testing.
pub fn build_argv(spec: &Egg) -> Vec<String> {
    let mut argv = vec![
        "--verbose".to_string(),
        "--model".to_string(), spec.model_id.clone(),
        "--dangerously-skip-permissions".to_string(),
        "--disable-slash-commands".to_string(),
    ];
    if let Some(mcp_url) = spec.mcp_url.as_deref() {
        let mcp_config = serde_json::json!({
            "mcpServers": {
                "hum": { "type": "http", "url": mcp_url }
            }
        }).to_string();
        argv.push("--mcp-config".into());
        argv.push(mcp_config);
        argv.push("--strict-mcp-config".into());
    }
    argv.push("--disallowed-tools".into());
    argv.push(HOST_TOOLS.join(" "));
    if let Some(sp) = spec.prompt.system_prompt.as_deref() {
        argv.push("--system-prompt".into());
        argv.push(sp.to_string());
    }
    if let Some(harness_sid) = spec.prompt.resume.as_deref() {
        argv.push("--resume".into());
        argv.push(harness_sid.to_string());
    }
    argv
}

pub fn claude_program() -> Result<std::path::PathBuf> {
    if let Some(named) = std::env::var_os("CLAUDE_CLI_PATH").filter(|v| !v.is_empty()) {
        return Ok(named.into());
    }
    let mut dirs = hum_paths::program_dirs();
    dirs.push(hum_paths::home().join(".claude/local"));
    hum_paths::find_program_in("claude", &dirs).ok_or_else(|| {
        let looked: Vec<String> = dirs.iter().map(|d| d.display().to_string()).collect();
        anyhow::anyhow!(
            "no claude program in {}; install Claude Code, or name it in CLAUDE_CLI_PATH",
            looked.join(":")
        )
    })
}

#[async_trait]
impl Nurse for ClaudeReplNurse {
    fn ephemeral(&self) -> bool {
        true
    }

    async fn raise(&self, spec: Egg) -> Result<Cell> {
        let cli = claude_program()?;
        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize {
                rows: 40,
                cols: 200,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("openpty")?;

        let mut cmd = CommandBuilder::new(&cli);
        cmd.cwd(&spec.cwd);
        if let Ok(path) = std::env::var("PATH") { cmd.env("PATH", path); }
        cmd.env("HOME", hum_paths::home().to_string_lossy().into_owned());
        cmd.env("TERM", "xterm-256color");
        cmd.env("CLAUDE_CODE_DISABLE_CLAUDE_MDS", "1");
        cmd.env("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1");
        cmd.env("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS", "1");
        cmd.env("CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING", "1");
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        for a in build_argv(&spec) {
            cmd.arg(a);
        }

        let mut child = pair.slave.spawn_command(cmd).with_context(|| format!("pty spawn {}", cli.display()))?;
        let pid = child.process_id();
        drop(pair.slave);
        if let Some(pid) = pid
            && let Err(e) = spec.bounds.apply_to(pid)
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e).context("apply resource limits");
        }

        let mut reader = pair.master.try_clone_reader().context("pty reader")?;
        let writer = pair.master.take_writer().context("pty writer")?;
        let writer = std::sync::Arc::new(std::sync::Mutex::new(writer));

        let (tx_in, mut rx_in) = mpsc::channel::<Value>(32);
        let (tx_evt, rx_evt) = mpsc::channel::<Value>(64);
        let (tx_exit, rx_exit) = oneshot::channel::<i32>();

        let writer_for_in = writer.clone();
        tokio::spawn(async move {
            while let Some(tone) = rx_in.recv().await {
                let text = words_of(&tone);
                if text.is_empty() {
                    continue;
                }
                if let Ok(mut w) = writer_for_in.lock() {
                    let _ = w.write_all(text.as_bytes());
                    let _ = w.write_all(b"\r");
                    let _ = w.flush();
                }
            }
        });

        // stdout reader — pumped on a blocking thread because portable-pty's
        // Reader is sync. State stays in-task: idle-2s OR sees `❯` → PERCHED.
        let tx_evt_out = tx_evt.clone();
        let harness_sid = spec.prompt.resume.clone().unwrap_or_default();
        let model = spec.model_id.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut acc = String::new();
            let mut state = HarnessState::Nesting;
            // v0 stub: reader.read() blocks, so a pure idle-timer in this
            // thread can't fire between reads. Real implementation would
            // poll on a non-blocking fd; for v0 the glyph-match below is
            // the only readiness signal.
            let prompt_glyph = '\u{276F}'; // ❯
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let s = String::from_utf8_lossy(&buf[..n]);
                        acc.push_str(&s);
                        if state == HarnessState::Nesting && acc.contains(prompt_glyph) {
                            state = HarnessState::Perched;
                            trace!(target: "nest", "pty.state Nesting->Perched (glyph)");
                            let _ = tx_evt_out.blocking_send(json!({
                                "chi": "pulse",
                                "kind": "cell-ready",
                                "cell": harness_sid,
                                "model": model,
                                "tools": [],
                            }));
                        }
                        // Drop accumulated buffer periodically; v0 doesn't
                        // synthesize transcript events from screen scrapes.
                        if acc.len() > 65536 {
                            acc.drain(0..acc.len() - 4096);
                        }
                    }
                    Err(e) => {
                        warn!(target: "nest", "pty.read.failed: {e}");
                        break;
                    }
                }
            }
            if state != HarnessState::Felled {
                trace!(target: "nest", "pty.read.eof state={:?}", state);
            }
        });

        // exit watcher — child.wait() is sync; run on blocking thread.
        std::thread::spawn(move || {
            let code = child
                .wait()
                .ok()
                .and_then(|s| s.exit_code().try_into().ok())
                .unwrap_or(1);
            let _ = tx_exit.send(code);
        });

        let cancel = tokio_util::sync::CancellationToken::new();
        let cancel_watch = cancel.clone();
        let mut master_holder = Some(pair.master);
        tokio::spawn(async move {
            cancel_watch.cancelled().await;
            master_holder.take();
        });

        trace!(target: "nest", "pty.spawned pid={:?}", pid);

        Ok(Cell {
            mark: pid,
            feed: tx_in,
            mmm: std::sync::Arc::new(Mutex::new(rx_evt)),
            emerged: rx_exit,
            ephemeral: true,
            silence: cancel,
        })
    }
}

fn words_of(tone: &Value) -> String {
    if tone.get("chi").and_then(Value::as_str) != Some("prompt") {
        return String::new();
    }
    let petals = tone.get("content").and_then(Value::as_array).cloned().unwrap_or_default();
    let words: Vec<&str> = petals
        .iter()
        .filter(|p| p.get("kind").and_then(Value::as_str) == Some("text"))
        .filter_map(|p| p.get("text").and_then(Value::as_str))
        .collect();
    if words.is_empty() {
        tone.get("text").and_then(Value::as_str).unwrap_or("").to_string()
    } else {
        words.join("\n")
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_cell_is_held_to_its_bounds() {
        let cli = std::env::temp_dir().join(format!("hum-repl-bounds-{}", std::process::id()));
        std::fs::write(&cli, "#!/bin/sh\nexec sleep 5\n").unwrap();
        std::fs::set_permissions(&cli, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        unsafe { std::env::set_var("CLAUDE_CLI_PATH", &cli) };
        let mut egg = Egg::new(hum_identity::HumId::mint(), "claude-haiku-4-5", std::env::temp_dir().to_string_lossy());
        egg.bounds = hum_nest::limits::Bounds { rss_bytes: Some(1 << 30), fd_count: Some(64), ..Default::default() };
        let cell = ClaudeReplNurse.raise(egg).await.expect("raise");
        let pid = cell.mark.expect("pid");
        let limits = std::fs::read_to_string(format!("/proc/{pid}/limits")).unwrap();
        cell.still();
        let _ = std::fs::remove_file(&cli);
        let of = |name: &str| limits.lines().find(|l| l.starts_with(name)).unwrap()[name.len()..]
            .split_whitespace().take(2).collect::<Vec<_>>().join(" ");
        assert_eq!(of("Max address space"), "1073741824 1073741824");
        assert_eq!(of("Max open files"), "64 64");
    }
}

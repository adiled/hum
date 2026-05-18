//! claude-repl — interactive `claude` over a PTY. v0 stub.
//!
//! Real behavior in TS lives in `nests/claude-repl/harness.ts`: a FSM
//! (NESTING → PERCHED → HUNTING → WILTING → HUSHED/FELLED), an ANSI/DEC
//! responder, hook FIFO, and JSONL transcript synth into stream-json.
//!
//! v0: spawn the PTY, watch stdout, mark PERCHED when the prompt glyph
//! `❯` shows up. No transcript synth, no hooks, no classifier. The cell
//! compiles and runs — it just can't carry a turn.

use std::io::Read;

use anyhow::{Context, Result};
use async_trait::async_trait;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{trace, warn};

use nest::{Cell, SpawnSpec, WorkerBee};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HarnessState {
    Nesting,
    Perched,
    #[allow(dead_code)]
    Hunting,
    #[allow(dead_code)]
    Wilting,
    #[allow(dead_code)]
    Hushed,
    Felled,
}

pub struct ClaudeReplWorker;

impl Default for ClaudeReplWorker {
    fn default() -> Self {
        Self
    }
}

/// Build the claude args for REPL/PTY mode. No `-p`, no `--input-format` —
/// interactive Ink TUI mode. Pure function for unit testing.
pub fn build_argv(spec: &SpawnSpec) -> Vec<String> {
    let mut argv = vec![
        "--verbose".to_string(),
        "--model".to_string(), spec.model_id.clone(),
        "--dangerously-skip-permissions".to_string(),
        "--disable-slash-commands".to_string(),
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
    if let Some(sp) = spec.system_prompt.as_deref() {
        argv.push("--system-prompt".into());
        argv.push(sp.to_string());
    }
    if let Some(harness_sid) = spec.resume_id.as_deref() {
        argv.push("--resume".into());
        argv.push(harness_sid.to_string());
    }
    argv
}

#[async_trait]
impl WorkerBee for ClaudeReplWorker {
    fn ephemeral(&self) -> bool {
        true
    }

    async fn spawn(&self, spec: SpawnSpec) -> Result<Cell> {
        let cli = spec.cli_path.clone().unwrap_or_else(|| "claude".into());
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
        if let Ok(home) = std::env::var("HOME") { cmd.env("HOME", home); }
        cmd.env("TERM", "xterm-256color");
        cmd.env("CLAUDE_CODE_DISABLE_CLAUDE_MDS", "1");
        cmd.env("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1");
        cmd.env("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS", "1");
        if !spec.plan_mode {
            cmd.env("CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING", "1");
        }
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        for a in build_argv(&spec) {
            cmd.arg(a);
        }

        let mut child = pair.slave.spawn_command(cmd).context("pty spawn")?;
        let pid = child.process_id();
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().context("pty reader")?;
        let writer = pair.master.take_writer().context("pty writer")?;
        let writer = std::sync::Arc::new(std::sync::Mutex::new(writer));

        let (tx_in, mut rx_in) = mpsc::channel::<String>(32);
        let (tx_evt, rx_evt) = mpsc::channel::<Value>(64);
        let (tx_exit, rx_exit) = oneshot::channel::<i32>();

        // stdin pump — interpret stream-json `user`/`text` lines as keystrokes.
        // Pure stub: extract text and write it followed by CR.
        let writer_for_in = writer.clone();
        tokio::spawn(async move {
            while let Some(line) = rx_in.recv().await {
                let text = prompt_text_from_json(&line).unwrap_or_default();
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
        let harness_sid = spec.resume_id.clone().unwrap_or_default();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut acc = String::new();
            let mut state = HarnessState::Nesting;
            // v0 stub: reader.read() blocks, so a pure idle-timer in this
            // thread can't fire between reads. Real implementation would
            // poll on a non-blocking fd; for v0 the glyph-match below is
            // the only readiness signal.
            let perched_signal = '\u{276F}'; // ❯
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let s = String::from_utf8_lossy(&buf[..n]);
                        acc.push_str(&s);
                        if state == HarnessState::Nesting && acc.contains(perched_signal) {
                            state = HarnessState::Perched;
                            trace!(target: "nest", "pty.state Nesting->Perched (glyph)");
                            let _ = tx_evt_out.blocking_send(json!({
                                "type": "system",
                                "subtype": "init",
                                "session_id": harness_sid,
                                "model": "claude",
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

        // master must outlive the writer; stash it behind Arc<Mutex<_>>
        // through a kill closure. We move the master into the closure.
        let master = std::sync::Arc::new(std::sync::Mutex::new(Some(pair.master)));
        let master_for_kill = master.clone();
        let kill_arc: std::sync::Arc<dyn Fn() + Send + Sync> =
            std::sync::Arc::new(move || {
                if let Ok(mut g) = master_for_kill.lock() {
                    // Dropping the master closes the PTY → child gets SIGHUP.
                    *g = None;
                }
            });

        trace!(target: "nest", "pty.spawned pid={:?}", pid);

        Ok(Cell {
            pid,
            stdin: tx_in,
            events: std::sync::Arc::new(Mutex::new(rx_evt)),
            exited: rx_exit,
            ephemeral: true,
            kill: kill_arc,
        })
    }
}

/// Pull `text` out of a stream-json `{type:"user", message:{content:[{text}]}}`
/// envelope. The PTY can only accept keystrokes, so non-text content (e.g.
/// tool_result) is dropped — the MCP server is the real round-trip path.
fn prompt_text_from_json(line: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line).ok()?;
    let content = v.get("message")?.get("content")?.as_array()?;
    let mut out = String::new();
    for part in content {
        if part.get("type")?.as_str()? == "text" {
            if let Some(t) = part.get("text").and_then(|x| x.as_str()) {
                out.push_str(t);
            }
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

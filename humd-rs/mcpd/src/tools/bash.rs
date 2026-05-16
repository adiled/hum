//! Bash — execute a shell command under the session cwd, with timeout
//! and output capping. The ban/write-block list mirrors mcp/tools.ts so
//! agents get the same rejection text on either backend.

use crate::protocol::{ToolDef, ToolResult};
use crate::session::SessionState;
use parking_lot::Mutex;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;

const BASH_MAX_OUTPUT: usize = 30_000;
const DEFAULT_TIMEOUT_MS: u64 = 120_000;

#[derive(Deserialize)]
struct Args {
    command: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    timeout: Option<u64>,
}

pub fn def() -> ToolDef {
    ToolDef {
        name: "Bash".to_string(),
        description: "Execute a shell command via /bin/bash -lc, in the session cwd. File-inspection commands (ls/cat/grep/find/etc.) are rejected — those go through Read/Glob/Grep. File-write commands (>, tee, cp, mv, rm, mkdir, touch, chmod) are rejected unless they're part of an allow-listed runtime (git, npm, cargo, …). Output is capped at 30KB per stream; default timeout 120s.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "command":     { "type": "string" },
                "description": { "type": "string" },
                "timeout":     { "type": "number", "description": "Milliseconds. Default 120000." },
            },
            "required": ["command"],
        }),
    }
}

pub async fn run(args: Value, session: &Arc<Mutex<SessionState>>) -> ToolResult {
    let args: Args = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return ToolResult::error(format!("invalid args: {e}")),
    };

    if let Some(rej) = check_ban(&args.command) {
        return rej;
    }
    if let Some(rej) = check_write(&args.command) {
        return rej;
    }

    let snap = session.lock().clone();
    if let Err(e) = snap.check_permission("Bash", Some(&args.command)) {
        return ToolResult::error(e);
    }

    let timeout_ms = args.timeout.unwrap_or(DEFAULT_TIMEOUT_MS);
    let mut cmd = Command::new("/bin/bash");
    cmd.arg("-lc")
        .arg(&args.command)
        .current_dir(&snap.cwd)
        .env("TERM", "dumb")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return ToolResult::error(format!("spawn failed: {e}")),
    };
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");

    let mut out_buf = Vec::with_capacity(8192);
    let mut err_buf = Vec::with_capacity(2048);
    let collect = async {
        let so = stdout.read_to_end(&mut out_buf);
        let se = stderr.read_to_end(&mut err_buf);
        let _ = tokio::join!(so, se);
        child.wait().await
    };

    let (interrupted, exit_code) = match timeout(Duration::from_millis(timeout_ms), collect).await {
        Ok(Ok(status)) => (false, status.code().unwrap_or(1)),
        Ok(Err(e)) => return ToolResult::error(format!("wait failed: {e}")),
        Err(_) => (true, 124),
    };

    let stdout_s = strip_ansi(&String::from_utf8_lossy(&out_buf));
    let stderr_s = strip_ansi(&String::from_utf8_lossy(&err_buf));
    let (out_cap, out_trim) = cap(&stdout_s);
    let (err_cap, err_trim) = cap(&stderr_s);

    let mut body = String::new();
    if !err_cap.is_empty() {
        if !out_cap.is_empty() {
            body.push_str(&out_cap);
            body.push('\n');
        }
        body.push_str("<stderr>\n");
        body.push_str(&err_cap);
        body.push_str("\n</stderr>");
    } else {
        body.push_str(&out_cap);
    }
    if interrupted {
        body = format!(
            "[hum: command interrupted after {timeout_ms}ms timeout — partial output follows]\n{body}"
        );
    }
    if body.is_empty() {
        body = format!("(exit {exit_code})");
    }

    ToolResult {
        output: body,
        title: Some(
            args.description
                .clone()
                .unwrap_or_else(|| args.command.chars().take(80).collect()),
        ),
        metadata: Some(json!({
            "exit": exit_code,
            "interrupted": interrupted,
            "stdoutTrimmed": out_trim,
            "stderrTrimmed": err_trim,
        })),
        is_error: false,
    }
}

fn cap(s: &str) -> (String, usize) {
    if s.len() <= BASH_MAX_OUTPUT {
        (s.to_string(), 0)
    } else {
        let kept = &s[..BASH_MAX_OUTPUT];
        let kb = BASH_MAX_OUTPUT / 1024;
        (
            format!("{kept}\n[... truncated at {kb}KB]"),
            s.len() - BASH_MAX_OUTPUT,
        )
    }
}

fn strip_ansi(s: &str) -> String {
    // CSI sequences: ESC '[' params... final-byte (0x40..0x7E).
    let re = Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").expect("static regex");
    re.replace_all(s, "").into_owned()
}

const BANNED: &[&str] = &[
    "ls", "find", "grep", "rg", "ripgrep", "cat", "head", "tail", "sed", "awk", "cut", "uniq",
    "wc", "more", "less", "tree", "du", "file", "od", "xxd", "strings", "zcat", "bzcat", "xzcat",
    "zgrep", "xargs",
];

fn first_token(segment: &str) -> Option<String> {
    let trimmed = segment.trim();
    if trimmed.is_empty() { return None; }
    let env_re = Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*=").unwrap();
    let mut tokens = trimmed.split_whitespace();
    let mut tok = tokens.next()?;
    while env_re.is_match(tok) {
        match tokens.next() {
            Some(t) => tok = t,
            None => return None,
        }
    }
    let clean: String = tok
        .trim_start_matches(['!', '`', '\'', '"'])
        .trim_end_matches(['`', '\'', '"'])
        .to_string();
    Some(clean.rsplit('/').next().unwrap_or(&clean).to_string())
}

fn check_ban(command: &str) -> Option<ToolResult> {
    let segments = Regex::new(r"\|\||&&|[|;\n]").unwrap();
    for seg in segments.split(command) {
        if let Some(tok) = first_token(seg) {
            if BANNED.contains(&tok.as_str()) {
                return Some(ToolResult {
                    output: format!(
                        "[hum: bash command '{tok}' is banned — file inspection must go through `Read`.\n\
                          ls / tree / du / file          -> Read(<directory>)\n\
                          find                           -> Read('<dir>') for a tree or Glob('<dir>/**/*.ext')\n\
                          cat / head / tail              -> Read(<file>)\n\
                          grep / rg / sed -n / awk       -> Grep(<file_or_dir>, pattern)\n\
                        Rewrite using the right tool and retry.]"
                    ),
                    title: Some(format!("banned: {tok}")),
                    metadata: Some(json!({ "banned": tok, "command": command })),
                    is_error: false,
                });
            }
        }
    }
    None
}

// Patterns that move bytes onto disk. Allow-listed runtimes (git, npm, …)
// short-circuit the check.
fn check_write(command: &str) -> Option<ToolResult> {
    let allow = [
        r"^\s*git\b",
        r"^\s*npm\b", r"^\s*yarn\b", r"^\s*pnpm\b", r"^\s*bun\b",
        r"^\s*pip\b", r"^\s*uv\b", r"^\s*cargo\b", r"^\s*go\b",
        r"^\s*make\b", r"^\s*cmake\b",
        r"^\s*docker\b", r"^\s*docker-compose\b",
        r"^\s*tsc\b", r"^\s*tsup\b", r"^\s*esbuild\b", r"^\s*vite\b", r"^\s*webpack\b",
        r"^\s*pytest\b", r"^\s*jest\b", r"^\s*vitest\b",
        r"^\s*rustc\b", r"^\s*gcc\b", r"^\s*g\+\+\b", r"^\s*clang\b",
    ];
    for pat in &allow {
        if Regex::new(pat).unwrap().is_match(command) {
            return None;
        }
    }
    let bad = [
        r"[^|&;]\s*>\s*[^&|]",
        r"[^|&;]\s*>>\s*",
        r"\btee\s",
        r"\bdd\s",
        r"\binstall\s+-",
        r"\bmkdir\s",
        r"\btouch\s",
        r"\bcp\s",
        r"\bmv\s",
        r"\bchmod\s",
        r"\bchown\s",
        r"\bln\s",
        r"\brm\s",
        r"\brmdir\s",
    ];
    for pat in &bad {
        if Regex::new(pat).unwrap().is_match(command) {
            return Some(ToolResult {
                output: "[hum: file writes go through Write/Edit, not Bash. Bash is for runtimes — git, builds, tests, package managers.]".into(),
                title: Some("bash write blocked".into()),
                metadata: Some(json!({ "command": command.chars().take(100).collect::<String>() })),
                is_error: false,
            });
        }
    }
    None
}

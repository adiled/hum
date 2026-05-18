//! `humfs_bash` — escape hatch shell tool.
//!
//! Two pre-flight filters that run before any execution:
//!
//! - **HARD BANNED commands** (`ls`, `find`, `grep`, `rg`, `cat`,
//!   `head`, `tail`, `sed`, `awk`, `cut`, `uniq`, `wc`, `more`,
//!   `less`, `tree`, `du`, `file`, `od`, `xxd`, `strings`, `zcat`,
//!   `bzcat`, `xzcat`, `zgrep`, `xargs`) — every file-inspection
//!   pattern goes through `humfs_read` instead. The filter splits on
//!   `|`, `&&`, `||`, `;`, newlines, strips env-var prefixes
//!   (`FOO=bar cat x`), leading punctuation, and uses the basename
//!   (`/usr/bin/cat` matches `cat`). Cannot be hidden in `bash -c`,
//!   `sh -c`, `env`, or shell functions — we filter the OUTER form.
//! - **Write-block patterns** (`>`, `>>`, `tee`, `dd`, `mkdir`,
//!   `touch`, `cp`, `mv`, `rm`, `chmod`, `chown`, `ln`, scripting-
//!   language file-write one-liners) — file authoring goes through
//!   `humfs_do_code` / `humfs_do_noncode`. Allow-list bypass for
//!   commands whose primary operation legitimately writes (git, npm,
//!   pnpm, yarn, bun, pip, uv, cargo, go, make, cmake, docker, tsc,
//!   pytest, jest, gcc, etc.).
//!
//! Execution: `/bin/bash -lc <cmd>` in the forager's cwd; stdout +
//! stderr ring-buffered at 30 KB per stream; default timeout
//! 120000ms; SIGTERM then SIGKILL on timeout.

use std::collections::HashSet;
use std::process::Stdio;
use std::time::Duration;

use nest_common::{ToolDef, ToolResult};
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;

const MAX_OUTPUT: usize = 30_000;
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
        name: "humfs_bash".into(),
        description: "Execute a shell command under the session cwd. Escape hatch for actions that aren't filesystem analysis or modification: running tests, git operations, build scripts, package managers, language runtimes, CLI utilities. HARD BANNED: ls/find/grep/rg/ripgrep/cat/head/tail/sed/awk/cut/uniq/wc/more/less/tree/du/file/od/xxd/strings/zcat/bzcat/xzcat/zgrep/xargs — these are file inspection and humfs_read handles them. Filter applies post-unwrap; bash -c / sh -c / env / shell functions cannot hide a banned command. File-writing commands (>, >>, tee, dd, cp, mv, rm, mkdir, touch, chmod, ln, scripting one-liners) are also blocked unless inside an allowlisted runtime invocation (git, npm/yarn/pnpm/bun, pip/uv/cargo/go, make, docker, tsc, pytest/jest, gcc/clang). Output capped at 30KB per stream; default timeout 120000ms.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "command":     { "type": "string" },
                "description": { "type": "string", "description": "Short description of what the command does." },
                "timeout":     { "type": "number", "description": "Milliseconds. Default 120000." },
            },
            "required": ["command"],
        }),
    }
}

pub async fn run(args: Value) -> ToolResult {
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

    let timeout_ms = args.timeout.unwrap_or(DEFAULT_TIMEOUT_MS);
    run_shell(&args.command, args.description.as_deref(), timeout_ms).await
}

// ── ban filter ─────────────────────────────────────────────────────────────

fn banned_set() -> HashSet<&'static str> {
    [
        "ls","find","grep","rg","ripgrep","cat","head","tail","sed","awk",
        "cut","uniq","wc","more","less","tree","du","file","od","xxd",
        "strings","zcat","bzcat","xzcat","zgrep","xargs",
    ].into_iter().collect()
}

/// Normalize a compound-command segment to its first executable
/// token. Strips leading `FOO=bar` env assignments, leading
/// punctuation, returns the basename so `/usr/bin/cat` matches
/// `cat`. Returns None if the segment carries no command.
fn first_command_token(segment: &str) -> Option<String> {
    let trimmed = segment.trim();
    if trimmed.is_empty() { return None; }
    let env_assign = Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*=").unwrap();
    let mut tokens = trimmed.split_whitespace();
    let mut tok = tokens.next()?;
    while env_assign.is_match(tok) {
        match tokens.next() {
            Some(t) => tok = t,
            None => return None,
        }
    }
    // Strip leading !`'" and trailing `'".
    let clean: String = tok
        .trim_start_matches(|c| matches!(c, '!' | '`' | '\'' | '"'))
        .trim_end_matches(|c| matches!(c, '`' | '\'' | '"'))
        .to_string();
    // basename
    let base = clean.rsplit('/').next().unwrap_or(&clean);
    if base.is_empty() { None } else { Some(base.to_string()) }
}

fn check_ban(command: &str) -> Option<ToolResult> {
    let split = Regex::new(r"\|\||&&|\||;|\n").unwrap();
    let banned = banned_set();
    for seg in split.split(command) {
        if let Some(tok) = first_command_token(seg) {
            if banned.contains(tok.as_str()) {
                return Some(ToolResult {
                    output: format!(
                        "[humfs: bash command '{tok}' is banned — file inspection must go through `humfs_read`.\n  ls / tree / du / file          → humfs_read(<directory>)\n  find                           → humfs_read('<dir>') for a tree, or humfs_read('<dir>/**/*.ext') as a glob\n  cat / head / tail / more / less → humfs_read(<file>)\n  grep / rg / sed -n / awk       → humfs_read(<file_or_dir>, pattern: 'regex')\nRewrite the call to use humfs_read(...) and try again. If you genuinely need a shell-only capability that bash actually provides (runtime, package managers, git, etc.), call that directly.]"
                    ),
                    title: Some(format!("banned: {tok}")),
                    metadata: Some(json!({ "banned": tok, "command": command })),
                    is_error: true,
                });
            }
        }
    }
    None
}

// ── write-block filter ────────────────────────────────────────────────────

fn write_patterns() -> Vec<Regex> {
    [
        r"[^|&;]\s*>\s*[^&|]",           // > file (not >&2)
        r"[^|&;]\s*>>\s*",               // >> file
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
        r"(?i)\bpython[23]?\s+-c\b.*(open|write|Path)",
        r"(?i)\bnode\s+-e\b.*(writeFile|fs\.)",
        r"(?i)\bruby\s+-e\b.*(File\.|IO\.)",
        r"(?i)\bperl\s+-[ep]\b.*(open|print)",
    ].iter().map(|p| Regex::new(p).unwrap()).collect()
}

fn write_allowed() -> Vec<Regex> {
    [
        r"^\s*git\s",
        r"^\s*npm\s", r"^\s*yarn\s", r"^\s*pnpm\s", r"^\s*bun\s",
        r"^\s*pip\s", r"^\s*uv\s", r"^\s*cargo\s", r"^\s*go\s",
        r"^\s*make\s", r"^\s*cmake\s",
        r"^\s*docker\s", r"^\s*docker-compose\s",
        r"^\s*tsc\b", r"^\s*tsup\b", r"^\s*esbuild\b", r"^\s*vite\b", r"^\s*webpack\b",
        r"^\s*pytest\b", r"^\s*jest\b", r"^\s*vitest\b",
        r"^\s*rustc\b", r"^\s*gcc\b", r"^\s*g\+\+\b", r"^\s*clang\b",
    ].iter().map(|p| Regex::new(p).unwrap()).collect()
}

fn check_write(command: &str) -> Option<ToolResult> {
    let allowed = write_allowed();
    if allowed.iter().any(|p| p.is_match(command)) {
        return None;
    }
    let blocks = write_patterns();
    for p in &blocks {
        if p.is_match(command) {
            return Some(ToolResult {
                output: "[humfs: file writes go through humfs_do_code (code files) or humfs_do_noncode (non-code files), not bash. Bash is for: git, builds (npm/make/tsc), tests (pytest/jest), package managers, and runtime commands. Rewrite using humfs_do_code or humfs_do_noncode.]".into(),
                title: Some("bash write blocked".into()),
                metadata: Some(json!({ "command": command.chars().take(100).collect::<String>() })),
                is_error: true,
            });
        }
    }
    None
}

// ── exec ──────────────────────────────────────────────────────────────────

async fn run_shell(command: &str, description: Option<&str>, timeout_ms: u64) -> ToolResult {
    let mut child = match Command::new("/bin/bash")
        .arg("-lc")
        .arg(command)
        .env("TERM", "dumb")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return ToolResult::error(format!("spawn failed: {e}")),
    };

    let mut stdout = child.stdout.take().expect("stdout piped");
    let mut stderr = child.stderr.take().expect("stderr piped");

    let pump_out = tokio::spawn(async move {
        let mut buf = String::new();
        let mut tmp = [0u8; 4096];
        loop {
            match stdout.read(&mut tmp).await {
                Ok(0) | Err(_) => break,
                Ok(n) => append_ring(&mut buf, &String::from_utf8_lossy(&tmp[..n])),
            }
        }
        buf
    });
    let pump_err = tokio::spawn(async move {
        let mut buf = String::new();
        let mut tmp = [0u8; 4096];
        loop {
            match stderr.read(&mut tmp).await {
                Ok(0) | Err(_) => break,
                Ok(n) => append_ring(&mut buf, &String::from_utf8_lossy(&tmp[..n])),
            }
        }
        buf
    });

    let mut interrupted = false;
    let exit_code = match timeout(Duration::from_millis(timeout_ms), child.wait()).await {
        Ok(Ok(status)) => status.code().unwrap_or(1),
        Ok(Err(e)) => {
            return ToolResult::error(format!("wait failed: {e}"));
        }
        Err(_) => {
            interrupted = true;
            let _ = child.start_kill();
            let _ = child.wait().await;
            124
        }
    };

    let stdout_kept = pump_out.await.unwrap_or_default();
    let stderr_kept = pump_err.await.unwrap_or_default();

    let stdout_kept = strip_ansi(&stdout_kept);
    let stderr_kept = strip_ansi(&stderr_kept);

    let mut body = String::new();
    if !stderr_kept.is_empty() {
        if !stdout_kept.is_empty() {
            body.push_str(&stdout_kept);
            body.push('\n');
        }
        body.push_str("<stderr>\n");
        body.push_str(&stderr_kept);
        body.push_str("\n</stderr>");
    } else {
        body.push_str(&stdout_kept);
    }
    if interrupted {
        body = format!(
            "[humfs: command interrupted after {timeout_ms}ms timeout — partial output follows]\n{body}"
        );
    }

    let title_default: String = command.chars().take(80).collect();
    let title = description.map(str::to_string).unwrap_or(title_default);

    ToolResult {
        output: if body.is_empty() { format!("(exit {exit_code})") } else { body },
        title: Some(title.clone()),
        metadata: Some(json!({
            "exit": exit_code,
            "description": title,
            "interrupted": interrupted,
            "stdoutBytes": stdout_kept.len(),
            "stderrBytes": stderr_kept.len(),
        })),
        is_error: false,
    }
}

// ── helpers ───────────────────────────────────────────────────────────────

/// Ring-buffer append: keeps the tail of the stream when total
/// exceeds MAX_OUTPUT. Useful for long-running commands where the
/// end matters more than the start.
fn append_ring(buf: &mut String, chunk: &str) {
    buf.push_str(chunk);
    if buf.len() > MAX_OUTPUT {
        let drop = buf.len() - MAX_OUTPUT;
        // Drop from the front, on a char boundary.
        let mut idx = drop;
        while !buf.is_char_boundary(idx) { idx += 1; }
        buf.drain(..idx);
    }
}

/// Strip CSI escape sequences (ANSI colors, cursor movement). Keeps
/// printable output. Conservative — only removes `ESC [ ... <final>`.
fn strip_ansi(s: &str) -> String {
    let re = Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap();
    re.replace_all(s, "").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ban_simple() {
        assert!(check_ban("ls -la").is_some());
        assert!(check_ban("cat /etc/passwd").is_some());
        assert!(check_ban("/usr/bin/grep foo bar").is_some());
    }

    #[test]
    fn ban_compound() {
        assert!(check_ban("echo hi | grep hi").is_some());
        assert!(check_ban("true && ls").is_some());
        assert!(check_ban("false || cat x").is_some());
    }

    #[test]
    fn ban_env_prefix() {
        assert!(check_ban("FOO=bar BAZ=qux cat /etc/passwd").is_some());
    }

    #[test]
    fn ban_lets_runtime_through() {
        assert!(check_ban("git status").is_none());
        assert!(check_ban("npm test").is_none());
        assert!(check_ban("cargo build").is_none());
    }

    #[test]
    fn write_block_simple() {
        assert!(check_write("echo hi > /tmp/x").is_some());
        assert!(check_write("touch /tmp/y").is_some());
        assert!(check_write("rm -rf /tmp/z").is_some());
    }

    #[test]
    fn write_block_allows_git_etc() {
        // git/npm/cargo legitimately write — should not be blocked.
        assert!(check_write("git commit -m 'x'").is_none());
        assert!(check_write("npm install").is_none());
        assert!(check_write("cargo build --release").is_none());
    }
}

//! `humfs_bash` — escape hatch shell tool. P0 stub.
//!
//! Hard-bans file-inspection commands (ls/find/grep/rg/cat/head/
//! tail/sed/awk/cut/sort -u/uniq/wc/more/less/tree/du/file/od/xxd/
//! strings) — agents are redirected back to `humfs_read`. Filter
//! checks post-unwrap so `bash -c`, `sh -c`, `env` can't hide a
//! banned command.
//!
//! Write-block list rejects bare `>`, `tee`, `cp`, `mv`, `rm`,
//! `mkdir`, `touch`, `chmod` unless inside an allowlisted runtime
//! invocation (git, npm, yarn, pnpm, bun, pip, uv, cargo, go, tsc,
//! make, pytest, jest, docker, systemctl, journalctl, curl, wget,
//! kill, ps).
//!
//! Output capped at 30KB per stream; default timeout 120000ms.
//!
//! Implementation lands in P1.

use nest_common::{ToolDef, ToolResult};
use serde_json::{json, Value};

pub fn def() -> ToolDef {
    ToolDef {
        name: "humfs_bash".into(),
        description: "Execute a shell command under the session cwd. Escape hatch for actions that aren't filesystem analysis or modification: running tests, git operations, build scripts, package managers, language runtimes, CLI utilities. HARD BANNED: ls/find/grep/rg/ripgrep/cat/head/tail/sed/awk/cut/sort -u/uniq/wc/more/less/tree/du/file/od/xxd/strings — these are file inspection and humfs_read handles them. Filter applies post-unwrap; bash -c / sh -c / env / shell functions cannot hide a banned command. Output capped at 30KB per stream; default timeout 120000ms.".into(),
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

pub async fn run(_args: Value) -> ToolResult {
    ToolResult::error("humfs_bash: not yet implemented (P0 skeleton; lands in P1)")
}

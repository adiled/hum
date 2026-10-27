//! claude-cli — the pipe-mode Nurse for claude.
//!
//! `claude -p --input-format stream-json --output-format stream-json`.
//! Takes a [`hum_nest::Egg`], builds the CLI invocation, runs the
//! subprocess, and is its cell: fed the sigil's tones and giving back the
//! tones of what claude grew, translated by [`stream`]. The nest never sees
//! claude's arguments or its events; this crate owns them.
//!
//! A cell never works the host it runs on. Claude Code's own host tools,
//! [`HOST_TOOLS`], are always turned off; its shell is the bloom's `bash`,
//! one of the tools the nest offers it over MCP, which a builder serves
//! wherever the bloom builds.
//!
//! A sigil's session is claude's, its id derived from the sid in
//! [`SESSION_NS`], its transcript under `~/.claude/projects`, found by the
//! nest's path with its links resolved as claude itself finds it. A cell
//! resumes the session when its transcript exists and begins it otherwise,
//! and resumes the session a prompt names in `resume` above both.
//!
//! A prompt's graft is stitched into the session's transcript before the
//! cell resumes it, so a cell that did not grow those blooms grows on as if
//! it had; a transcript that already holds them is left as it is.
//!
//! claude signed in with a subscription takes no system prompt of a
//! caller's: `--system-prompt` and `--append-system-prompt` never reach the
//! model. So a session's system prompt is told once, at the head of its
//! first user turn, [`instructions`] wrapping it: in the turn fed to a new
//! session, or in the first turn of a graft stitched into one. A session
//! whose transcript already holds its first turn is not told again.
//!
//! How the bloom grows reaches claude where its CLI can take it: a
//! `trellis` is its `--json-schema`, `"json"` being any object, and a
//! `sepal` effort its `--effort`, an effort below low taken as low. A
//! prompt's `allowedTools` are its `--allowed-tools`.

pub mod graft;
pub mod stream;

use std::process::Stdio;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use command_group::AsyncCommandGroup;
use tokio::process::Command;
use tokio::sync::{mpsc, Mutex};
use tracing::{trace, warn};
use uuid::Uuid;

use hum_nest::{Propensity, Cell, Egg, Nurse};

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

pub const HOST_TOOLS: &[&str] = &[
    "Bash", "BashOutput", "KillShell", "KillBash",
    "Read", "Write", "Edit", "MultiEdit", "NotebookEdit",
    "Glob", "Grep", "LS",
];

pub const SESSION_NS: Uuid = Uuid::from_bytes([
    0x68, 0x75, 0x6d, 0x2d, 0x73, 0x65, 0x73, 0x73, 0x69, 0x6f, 0x6e, 0x2d, 0x6e, 0x73, 0x00, 0x01,
]);

pub struct ClaudeCliNurse;

impl Default for ClaudeCliNurse {
    fn default() -> Self { Self }
}

pub fn session_of(sid: &hum_identity::HumId) -> String {
    sid.to_uuid_v5(SESSION_NS).to_string()
}

pub enum Session<'a> {
    Resume(&'a str),
    Begin(&'a str),
}

pub fn build_argv(spec: &Egg, session: Session<'_>) -> Vec<String> {
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
                "hum": { "type": "http", "url": mcp_url }
            }
        }).to_string();
        argv.push("--mcp-config".into());
        argv.push(mcp_config);
        argv.push("--strict-mcp-config".into());
    }
    if let Some(allowed) = spec.prompt.allowed_tools.as_ref().filter(|a| !a.is_empty()) {
        argv.push("--allowed-tools".into());
        argv.push(allowed.join(" "));
    }
    argv.push("--disallowed-tools".into());
    argv.push(HOST_TOOLS.join(" "));
    if let Some(sp) = spec.prompt.system_prompt.as_deref().filter(|s| !s.is_empty()) {
        argv.push("--system-prompt".into());
        argv.push(sp.to_string());
    }
    match &spec.prompt.trellis {
        Some(schema @ Value::Object(_)) => {
            argv.push("--json-schema".into());
            argv.push(schema.to_string());
        }
        Some(Value::String(shape)) if shape == "json" => {
            argv.push("--json-schema".into());
            argv.push(r#"{"type":"object"}"#.into());
        }
        _ => {}
    }
    if let Some(effort) = spec.prompt.sepal.as_ref().and_then(|s| s.effort.as_deref()) {
        argv.push("--effort".into());
        argv.push(match effort {
            "medium" | "high" | "xhigh" | "max" => effort.to_string(),
            _ => "low".to_string(),
        });
    }
    match session {
        Session::Resume(id) => {
            argv.push("--resume".into());
            argv.push(id.to_string());
        }
        Session::Begin(id) => {
            argv.push("--session-id".into());
            argv.push(id.to_string());
        }
    }
    argv
}

pub fn instructions(system: &str) -> String {
    format!("<system>\n{system}\n</system>\nThose are the instructions for this whole conversation, as its system prompt.")
}

pub fn build_env(spec: &Egg) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = vec![
        ("CLAUDE_CODE_DISABLE_CLAUDE_MDS".into(), "1".into()),
        ("CLAUDE_CODE_DISABLE_AUTO_MEMORY".into(), "1".into()),
        ("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS".into(), "1".into()),
        ("CLAUDE_CODE_DISABLE_FAST_MODE".into(), "1".into()),
        ("DISABLE_INTERLEAVED_THINKING".into(), "1".into()),
        ("ENABLE_TOOL_SEARCH".into(), "false".into()),
        ("CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING".into(), "1".into()),
    ];
    if let Ok(path) = std::env::var("PATH") {
        env.push(("PATH".into(), path));
    }
    env.push(("HOME".into(), hum_paths::home().to_string_lossy().into_owned()));
    for (k, v) in &spec.env {
        env.push((k.clone(), v.clone()));
    }
    env
}

#[async_trait]
impl Nurse for ClaudeCliNurse {
    fn ephemeral(&self) -> bool { false }
    fn propensity(&self) -> Propensity { Propensity::StatefulSession }

    async fn curate(&self, sid: &hum_identity::HumId, cwd: &str) -> Result<hum_nest::CurateReport> {
        let derived = session_of(sid);
        let cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.into());
        let path = graft::session_path(&cwd, &derived);
        if !path.exists() {
            trace!(sid = %sid, path = %path.display(), "nurse.curate.no-transcript");
            return Ok(hum_nest::CurateReport::default());
        }

        let pruned = graft::prune_jsonl(&path)?;
        trace!(
            sid = %sid,
            trimmed = pruned.trimmed,
            stripped = pruned.stripped,
            bytes_before = pruned.bytes_before,
            bytes_after = pruned.bytes_after,
            "nurse.curate.pruned"
        );

        Ok(hum_nest::CurateReport {
            bytes_before: pruned.bytes_before as u64,
            bytes_after: pruned.bytes_after as u64,
        })
    }

    async fn raise(&self, spec: Egg) -> Result<Cell> {
        let derived = session_of(&spec.sid);
        let cwd = std::fs::canonicalize(&spec.cwd).unwrap_or_else(|_| spec.cwd.clone().into());
        let path = graft::session_path(&cwd, &derived);
        let foreign = spec.prompt.resume.clone().filter(|r| !r.is_empty());
        let begun = foreign.is_some() || path.exists();
        let mut preamble = spec.prompt.system_prompt.as_deref().filter(|s| !s.is_empty() && !begun).map(instructions);
        if foreign.is_none()
            && let Some(blooms) = spec.prompt.graft.as_ref().filter(|g| !g.is_empty())
        {
            if !path.exists() {
                graft::create_claude_session(&cwd, &derived)?;
            }
            let mut prior = graft::prior_of(blooms);
            if preamble.as_deref().is_some_and(|text| graft::tell(&mut prior, text)) {
                preamble = None;
            }
            let stitched = graft::graft(&prior, &path, &derived, &cwd, None)?;
            trace!(sid = %spec.sid, blooms = blooms.len(), grafted = stitched.grafted, "nurse.graft.stitched");
        }
        let session = match foreign.as_deref() {
            Some(id) => Session::Resume(id),
            None if path.exists() => Session::Resume(&derived),
            None => Session::Begin(&derived),
        };
        let cli = claude_program()?;
        let argv = build_argv(&spec, session);
        let env = build_env(&spec);
        let mut grower = stream::Grower::new(spec.prompt.trellis.is_some());

        let mut cmd = Command::new(&cli);
        cmd.args(&argv)
            .current_dir(&spec.cwd)
            .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        spec.bounds.apply_pre_exec(cmd.as_std_mut())
            .with_context(|| "apply resource limits")?;

        let mut child = cmd.group_spawn().with_context(|| format!("spawn {}", cli.display()))?;
        let pid = child.inner().id();
        let mut stdin = child.inner().stdin.take().context("missing stdin")?;
        let stdout = child.inner().stdout.take().context("missing stdout")?;
        let stderr = child.inner().stderr.take().context("missing stderr")?;

        let (tx_in, mut rx_in) = mpsc::channel::<Value>(64);
        let (tx_evt, rx_evt) = mpsc::channel::<Value>(256);

        tokio::spawn(async move {
            while let Some(tone) = rx_in.recv().await {
                let Some(line) = stream::heard(&tone, &mut preamble) else { continue };
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

        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            loop {
                match reader.next_line().await {
                    Ok(Some(line)) => {
                        let line = line.trim();
                        if line.is_empty() { continue; }
                        match serde_json::from_str::<Value>(line) {
                            Ok(event) => {
                                for tone in grower.grew(event) {
                                    if tx_evt.send(tone).await.is_err() { return; }
                                }
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

        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let line = line.trim();
                if !line.is_empty() {
                    trace!(target: "claude-cli", "stderr: {line}");
                }
            }
        });

        let (rx_exit, cancel) = hum_nest::lifecycle::tend(child);

        trace!(target: "claude-cli", "spawned pid={:?}", pid);

        Ok(Cell {
            mark: pid,
            feed: tx_in,
            mmm: std::sync::Arc::new(Mutex::new(rx_evt)),
            emerged: rx_exit,
            ephemeral: false,
            silence: cancel,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn egg() -> Egg {
        Egg::new(hum_identity::HumId::mint(), "claude-haiku-4-5", "/tmp")
    }

    #[test]
    fn argv_includes_basics() {
        let argv = build_argv(&egg(), Session::Begin("s"));
        assert!(argv.contains(&"-p".to_string()));
        assert!(argv.contains(&"--verbose".to_string()));
        assert!(argv.contains(&"claude-haiku-4-5".to_string()));
        assert!(argv.contains(&"--dangerously-skip-permissions".to_string()));
        assert!(argv.iter().any(|a| a == "stream-json"));
    }

    #[test]
    fn argv_turns_off_the_host_tools_always() {
        let argv = build_argv(&egg(), Session::Begin("s"));
        let i = argv.iter().position(|a| a == "--disallowed-tools").expect("host tools are always off");
        let off: Vec<&str> = argv[i + 1].split(' ').collect();
        for tool in HOST_TOOLS {
            assert!(off.contains(tool), "{tool} is off");
        }
    }

    #[test]
    fn argv_names_mcp_only_when_the_nest_gives_it() {
        let mut spec = egg();
        assert!(!build_argv(&spec, Session::Begin("s")).iter().any(|a| a == "--mcp-config"));
        spec.mcp_url = Some("http://127.0.0.1:29147/s/oai-1".into());
        let argv = build_argv(&spec, Session::Begin("s"));
        let idx = argv.iter().position(|a| a == "--mcp-config").expect("mcp-config flag");
        let config: serde_json::Value = serde_json::from_str(&argv[idx + 1]).unwrap();
        assert_eq!(config["mcpServers"]["hum"]["url"], "http://127.0.0.1:29147/s/oai-1");
        assert!(argv.iter().any(|a| a == "--strict-mcp-config"));
    }

    #[test]
    fn argv_takes_the_prompts_system_prompt_and_allowed_tools() {
        let mut spec = egg();
        spec.prompt.system_prompt = Some("Be terse.".into());
        spec.prompt.allowed_tools = Some(vec!["mcp__hum__bash".into()]);
        let argv = build_argv(&spec, Session::Begin("s"));
        let i = argv.iter().position(|a| a == "--system-prompt").unwrap();
        assert_eq!(argv[i + 1], "Be terse.");
        let a = argv.iter().position(|a| a == "--allowed-tools").unwrap();
        assert_eq!(argv[a + 1], "mcp__hum__bash");
    }

    #[test]
    fn a_trellis_and_an_effort_reach_the_cli() {
        let mut spec = egg();
        spec.prompt.trellis = Some(serde_json::json!("json"));
        spec.prompt.sepal = Some(thrum_core::Sepal { effort: Some("minimal".into()), budget: None });
        let argv = build_argv(&spec, Session::Begin("s"));
        let i = argv.iter().position(|a| a == "--json-schema").expect("a trellis is a schema");
        assert_eq!(argv[i + 1], r#"{"type":"object"}"#);
        let e = argv.iter().position(|a| a == "--effort").expect("an effort");
        assert_eq!(argv[e + 1], "low");
    }

    #[test]
    fn a_session_is_resumed_or_begun() {
        let argv = build_argv(&egg(), Session::Resume("abc-123"));
        let i = argv.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(argv[i + 1], "abc-123");
        let argv = build_argv(&egg(), Session::Begin("def-456"));
        let i = argv.iter().position(|a| a == "--session-id").unwrap();
        assert_eq!(argv[i + 1], "def-456");
    }

    #[test]
    fn a_session_id_is_derived_from_the_sid_alone() {
        let sid = hum_identity::HumId::mint();
        assert_eq!(session_of(&sid), session_of(&sid));
        assert_eq!(sid.to_uuid_v5(SESSION_NS).get_version_num(), 5);
    }

    #[test]
    fn env_turns_off_what_a_cell_must_not_do() {
        let env = build_env(&egg());
        assert!(env.iter().any(|(k, v)| k == "CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING" && v == "1"));
        assert!(env.iter().any(|(k, v)| k == "ENABLE_TOOL_SEARCH" && v == "false"));
    }

    #[test]
    fn env_user_override_wins() {
        let mut spec = egg();
        spec.env.insert("CLAUDE_CODE_DISABLE_FAST_MODE".into(), "0".into());
        let env = build_env(&spec);
        let last = env.iter().rev().find(|(k, _)| k == "CLAUDE_CODE_DISABLE_FAST_MODE").unwrap();
        assert_eq!(last.1, "0");
    }
}

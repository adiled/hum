//! graft — JSONL stitching for Claude CLI transcripts.
//!
//! Mirrors the TypeScript reference at `fs/session.ts`. Each function walks
//! `serde_json::Value` by field name so we don't have to model the whole
//! tagged-union shape statically.
//!
//! Files live at `<HOME>/.claude/projects/<dashed-cwd>/<session-id>.jsonl`.
//! Entry types we care about: `summary`, `user`, `assistant`, `last-prompt`,
//! `queue-operation`. Every real entry has `uuid` and `parentUuid`.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::trace;

// ─── Path resolution ──────────────────────────────────────────────────────

/// Translate a cwd into Claude's project-dir slug:
/// replace every non-alphanumeric char with `-`, then strip trailing dashes.
fn cwd_hash(cwd: &Path) -> String {
    let s = cwd.to_string_lossy();
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

fn claude_base() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".claude")
}

pub fn session_dir(cwd: &Path) -> PathBuf {
    claude_base().join("projects").join(cwd_hash(cwd))
}

pub fn session_path(cwd: &Path, session_id: &str) -> PathBuf {
    session_dir(cwd).join(format!("{session_id}.jsonl"))
}

/// Touch a fresh JSONL with a single `summary` skeleton entry.
pub fn create_claude_session(cwd: &Path, session_id: &str) -> Result<PathBuf> {
    let dir = session_dir(cwd);
    fs::create_dir_all(&dir).with_context(|| format!("mkdir {}", dir.display()))?;
    let path = session_path(cwd, session_id);
    let summary = json!({
        "type": "summary",
        "summary": "hum session",
        "leafUuid": Value::Null,
        "sessionId": session_id,
        "timestamp": now_iso(),
    });
    let mut line = serde_json::to_string(&summary)?;
    line.push('\n');
    fs::write(&path, line).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

// ─── Helpers ──────────────────────────────────────────────────────────────

fn now_iso() -> String {
    // Crude ISO-8601 millis-precision in UTC without pulling chrono.
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs() as i64;
    let ms = dur.subsec_millis();
    let (y, mo, d, h, mi, s) = epoch_to_civil(secs);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z", y, mo, d, h, mi, s, ms)
}

/// Howard Hinnant's days_from_civil inverse. secs is seconds since 1970-01-01.
fn epoch_to_civil(secs: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let h = (rem / 3600) as u32;
    let mi = ((rem % 3600) / 60) as u32;
    let s = (rem % 60) as u32;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if mo <= 2 { y + 1 } else { y };
    (y as i32, mo, d, h, mi, s)
}

fn read_entries(path: &Path) -> Vec<Value> {
    let Ok(bytes) = fs::read_to_string(path) else { return Vec::new() };
    let mut out = Vec::new();
    for line in bytes.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(mut v) = serde_json::from_str::<Value>(line) {
            // Coerce string content into [{type:text,text:...}] like the TS does.
            if let Some(msg) = v.get_mut("message") {
                if let Some(s) = msg.get("content").and_then(Value::as_str).map(str::to_owned) {
                    msg["content"] = json!([{ "type": "text", "text": s }]);
                }
            }
            out.push(v);
        }
    }
    out
}

fn write_entries(path: &Path, entries: &[Value]) -> Result<()> {
    let mut s = String::new();
    for e in entries {
        s.push_str(&serde_json::to_string(e)?);
        s.push('\n');
    }
    fs::write(path, s).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn entry_type(v: &Value) -> &str {
    v.get("type").and_then(Value::as_str).unwrap_or("")
}

fn message_content(v: &Value) -> Option<&Vec<Value>> {
    v.get("message").and_then(|m| m.get("content")).and_then(Value::as_array)
}

fn message_content_mut(v: &mut Value) -> Option<&mut Vec<Value>> {
    v.get_mut("message")
        .and_then(|m| m.get_mut("content"))
        .and_then(Value::as_array_mut)
}

// ─── last_uuid ────────────────────────────────────────────────────────────

pub fn last_uuid(path: &Path) -> Option<String> {
    let s = fs::read_to_string(path).ok()?;
    for line in s.lines().rev() {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if let Some(u) = v.get("uuid").and_then(Value::as_str) {
                return Some(u.to_owned());
            }
        }
    }
    None
}

// ─── Graft ────────────────────────────────────────────────────────────────

pub struct GraftResult {
    pub grafted: usize,
    pub last_petal: Option<String>,
}

fn role_of(petal: &Value) -> &str {
    petal.get("role").and_then(Value::as_str).unwrap_or("")
}

/// Count completed turns in priorPetals: a user msg followed by any non-user.
fn count_turns(messages: &[Value]) -> usize {
    let mut turns = 0;
    for i in 0..messages.len() {
        if role_of(&messages[i]) == "user"
            && i + 1 < messages.len()
            && role_of(&messages[i + 1]) != "user"
        {
            turns += 1;
        }
    }
    turns
}

/// Count completed turns in JSONL: user-with-text followed by an assistant
/// before another user.
fn count_jsonl_turns(entries: &[Value]) -> usize {
    let mut turns = 0;
    for i in 0..entries.len() {
        if entry_type(&entries[i]) != "user" {
            continue;
        }
        let has_text = message_content(&entries[i])
            .map(|c| c.iter().any(|p| p.get("type").and_then(Value::as_str) == Some("text")))
            .unwrap_or(false);
        if !has_text {
            continue;
        }
        for j in (i + 1)..entries.len() {
            match entry_type(&entries[j]) {
                "assistant" => {
                    turns += 1;
                    break;
                }
                "user" => break,
                _ => continue,
            }
        }
    }
    turns
}

fn skip_turns(messages: &[Value], n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let mut skipped = 0;
    let mut i = 0;
    while i < messages.len() {
        if role_of(&messages[i]) == "user"
            && i + 1 < messages.len()
            && role_of(&messages[i + 1]) != "user"
        {
            skipped += 1;
            if skipped >= n {
                let mut j = i + 1;
                while j < messages.len() && role_of(&messages[j]) != "user" {
                    j += 1;
                }
                return j;
            }
        }
        i += 1;
    }
    messages.len()
}

/// Stitch a nestler's prior petals into the JSONL. UUID-anchored,
/// count-based — does not hash text. Returns the new tail uuid.
pub fn graft(
    prior_petals: &[Value],
    jsonl_path: &Path,
    session_id: &str,
    cwd: &Path,
    last_synced: Option<&str>,
) -> Result<GraftResult> {
    // Strip system messages and a trailing user (murmur handles current prompt).
    let conversation: Vec<Value> = prior_petals
        .iter()
        .filter(|m| role_of(m) != "system")
        .cloned()
        .collect();
    let history: Vec<Value> =
        if conversation.last().map(role_of) == Some("user") {
            conversation[..conversation.len() - 1].to_vec()
        } else {
            conversation
        };

    if history.is_empty() || history.iter().all(|m| role_of(m) == "user") {
        let tail = last_synced.map(str::to_owned).or_else(|| last_uuid(jsonl_path));
        return Ok(GraftResult { grafted: 0, last_petal: tail });
    }

    let existing = read_entries(jsonl_path);
    let j_users = count_jsonl_turns(&existing);
    let p_users = count_turns(&history);

    let anchored = last_synced.is_some_and(|anchor| {
        existing
            .iter()
            .any(|e| e.get("uuid").and_then(Value::as_str) == Some(anchor))
    });

    if anchored && j_users >= p_users {
        trace!(anchor = ?last_synced, j_users, p_users, "graft.synced");
        return Ok(GraftResult { grafted: 0, last_petal: last_synced.map(str::to_owned) });
    }
    if j_users >= p_users {
        trace!(j_users, p_users, "graft.noop");
        return Ok(GraftResult { grafted: 0, last_petal: last_uuid(jsonl_path) });
    }

    let delta_start = skip_turns(&history, j_users);
    let delta = &history[delta_start..];
    trace!(j_users, p_users, delta_start, delta_len = delta.len(), "graft.delta");

    if delta.is_empty() || delta.iter().all(|m| role_of(m) == "user") {
        return Ok(GraftResult { grafted: 0, last_petal: last_uuid(jsonl_path) });
    }

    let count = append_from_prompt(jsonl_path, session_id, delta, cwd)?;
    Ok(GraftResult { grafted: count, last_petal: last_uuid(jsonl_path) })
}

/// Append AI-SDK-prompt-shaped messages to the JSONL as Claude entries.
/// Returns the number of assistant turns written. Emits the minimum shape
/// Claude CLI needs (uuid/parentUuid chain, type, role, content); full
/// fidelity (real version/gitBranch/usage stats) still lives in the TS writer.
fn append_from_prompt(
    path: &Path,
    session_id: &str,
    history: &[Value],
    cwd: &Path,
) -> Result<usize> {
    let mut parent_uuid = last_uuid(path);
    let mut assistant_count = 0;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    let cwd_s = cwd.to_string_lossy().into_owned();

    for msg in history {
        let role = role_of(msg);
        let raw = msg.get("content");
        let content: Vec<Value> = build_content(role, raw);
        if content.is_empty() {
            continue;
        }

        let uuid = random_uuid_v4();
        let ts = now_iso();
        let entry = match role {
            "user" | "tool" => json!({
                "type": "user",
                "uuid": uuid,
                "parentUuid": parent_uuid,
                "sessionId": session_id,
                "isSidechain": false,
                "timestamp": ts,
                "promptId": random_uuid_v4(),
                "message": { "role": "user", "content": content },
                "permissionMode": "default",
                "userType": "external",
                "entrypoint": "sdk-cli",
                "version": "2.1.86",
                "gitBranch": "main",
                "cwd": cwd_s,
            }),
            "assistant" => {
                assistant_count += 1;
                json!({
                    "type": "assistant",
                    "uuid": uuid,
                    "parentUuid": parent_uuid,
                    "sessionId": session_id,
                    "isSidechain": false,
                    "timestamp": ts,
                    "requestId": format!("req_01{}", uuid.replace('-', "")),
                    "message": {
                        "model": "claude-sonnet-4-5-20250929",
                        "id": format!("msg_01{}", uuid.replace('-', "")),
                        "type": "message",
                        "role": "assistant",
                        "content": content,
                        "stop_reason": "end_turn",
                        "stop_sequence": Value::Null,
                        "usage": {
                            "input_tokens": 0,
                            "output_tokens": 0,
                            "cache_creation_input_tokens": 0,
                            "cache_read_input_tokens": 0,
                        },
                    },
                    "userType": "external",
                    "entrypoint": "sdk-cli",
                    "version": "2.1.86",
                    "gitBranch": "main",
                    "cwd": cwd_s,
                })
            }
            _ => continue,
        };

        let line = serde_json::to_string(&entry)?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        parent_uuid = Some(uuid);
    }
    Ok(assistant_count)
}

fn build_content(role: &str, raw: Option<&Value>) -> Vec<Value> {
    let mut out = Vec::new();
    match (role, raw) {
        (_, None) => {}
        ("user", Some(Value::String(s))) => out.push(json!({ "type": "text", "text": s })),
        ("assistant", Some(Value::String(s))) if !s.is_empty() => {
            out.push(json!({ "type": "text", "text": s }))
        }
        (_, Some(Value::Array(parts))) => {
            for p in parts {
                let t = p.get("type").and_then(Value::as_str).unwrap_or("");
                match t {
                    "text" => {
                        if let Some(s) = p.get("text").and_then(Value::as_str) {
                            out.push(json!({ "type": "text", "text": s }));
                        }
                    }
                    "tool-call" => {
                        if let (Some(id), Some(name)) = (
                            p.get("toolCallId").and_then(Value::as_str),
                            p.get("toolName").and_then(Value::as_str),
                        ) {
                            let input = match p.get("input") {
                                Some(Value::String(s)) => {
                                    serde_json::from_str::<Value>(s).unwrap_or(json!({}))
                                }
                                Some(other) => other.clone(),
                                None => json!({}),
                            };
                            out.push(json!({
                                "type": "tool_use",
                                "id": id,
                                "name": name,
                                "input": input,
                            }));
                        }
                    }
                    "tool-result" => {
                        if let Some(id) = p.get("toolCallId").and_then(Value::as_str) {
                            let result = match p.get("result") {
                                Some(Value::String(s)) => s.clone(),
                                Some(other) => other.to_string(),
                                None => String::new(),
                            };
                            out.push(json!({
                                "type": "tool_result",
                                "tool_use_id": id,
                                "content": result,
                            }));
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    if role == "assistant" && out.is_empty() {
        out.push(json!({ "type": "text", "text": "(no text response)" }));
    }
    out
}

/// Tiny UUID-v4 emitter — avoids the `uuid` crate dep.
fn random_uuid_v4() -> String {
    let mut bytes = [0u8; 16];
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u128;
    let mut salt: u128 = t ^ ((&bytes as *const _) as u128).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    for b in &mut bytes {
        salt = salt.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *b = (salt >> 56) as u8;
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant 1
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    )
}

// ─── Sanitize ─────────────────────────────────────────────────────────────

pub struct SanitizeResult {
    pub removed: usize,
    pub fixed: usize,
    pub rules: Vec<String>,
}

fn text_of(v: &Value) -> String {
    message_content(v)
        .map(|cs| {
            cs.iter()
                .filter(|c| c.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|c| c.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

fn is_pure_tool_result(v: &Value) -> bool {
    if entry_type(v) != "user" {
        return false;
    }
    message_content(v)
        .map(|cs| {
            !cs.is_empty()
                && cs
                    .iter()
                    .all(|c| c.get("type").and_then(Value::as_str) == Some("tool_result"))
        })
        .unwrap_or(false)
}

pub fn sanitize_jsonl(path: &Path) -> Result<SanitizeResult> {
    let entries = read_entries(path);
    if entries.is_empty() {
        return Ok(SanitizeResult { removed: 0, fixed: 0, rules: vec![] });
    }
    let mut rules: Vec<String> = Vec::new();
    let original = entries.len();
    let mut clean: Vec<Value> = Vec::with_capacity(entries.len());

    // Pass 1: ghosts, API errors, empty tool_results.
    let mut i = 0;
    while i < entries.len() {
        let e = &entries[i];
        let next = entries.get(i + 1);

        if entry_type(e) == "user" {
            let text = text_of(e);
            if text.trim() == "Continue from where you left off."
                && next.map(entry_type) == Some("assistant")
            {
                let nt = text_of(next.unwrap());
                if nt.contains("No response requested") {
                    rules.push("ghost".into());
                    i += 2;
                    continue;
                }
            }
        }

        if entry_type(e) == "assistant" {
            let text = text_of(e);
            if text.contains("API Error:") {
                rules.push("api-error".into());
                i += 1;
                continue;
            }
        }

        let mut e_mut = e.clone();
        if entry_type(&e_mut) == "user" {
            let mut did_fix = false;
            if let Some(cs) = message_content_mut(&mut e_mut) {
                for c in cs.iter_mut() {
                    if c.get("type").and_then(Value::as_str) == Some("tool_result") {
                        let body = c.get("content").and_then(Value::as_str).unwrap_or("");
                        if body.is_empty() || body == "[Old tool result content cleared]" {
                            c["content"] = json!("(tool result unavailable)");
                            did_fix = true;
                        }
                    }
                }
            }
            if did_fix {
                rules.push("empty-result".into());
            }
        }
        clean.push(e_mut);
        i += 1;
    }

    // Pass 2: trim trailing junk.
    loop {
        let Some(last) = clean.last() else { break };
        if entry_type(last) == "assistant" {
            let has_tool_use = message_content(last)
                .map(|cs| {
                    cs.iter()
                        .any(|c| c.get("type").and_then(Value::as_str) == Some("tool_use"))
                })
                .unwrap_or(false);
            if has_tool_use {
                clean.pop();
                rules.push("trailing-tool-use".into());
                continue;
            }
        }
        if matches!(entry_type(last), "last-prompt" | "queue-operation") {
            clean.pop();
            continue;
        }
        break;
    }

    // Pass 2.5: coalesce adjacent pure-tool_result user entries.
    let mut merged: Vec<Value> = Vec::with_capacity(clean.len());
    let mut k = 0;
    while k < clean.len() {
        if is_pure_tool_result(&clean[k]) {
            let mut anchor = clean[k].clone();
            while k + 1 < clean.len() && is_pure_tool_result(&clean[k + 1]) {
                let extra = clean[k + 1].clone();
                let extra_content = extra
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if let Some(arr) = message_content_mut(&mut anchor) {
                    arr.extend(extra_content);
                }
                rules.push("merge-tool-results".into());
                k += 1;
            }
            merged.push(anchor);
            k += 1;
            continue;
        }
        merged.push(clean[k].clone());
        k += 1;
    }

    // Pass 3: validate tool_use ↔ tool_result pairing.
    let mut validated: Vec<Value> = Vec::with_capacity(merged.len());
    let mut m = 0;
    while m < merged.len() {
        let e = &merged[m];
        if entry_type(e) == "assistant" {
            let tool_use_ids: Vec<String> = message_content(e)
                .map(|cs| {
                    cs.iter()
                        .filter(|c| c.get("type").and_then(Value::as_str) == Some("tool_use"))
                        .filter_map(|c| c.get("id").and_then(Value::as_str).map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();

            if !tool_use_ids.is_empty() {
                let next = merged.get(m + 1);
                if let Some(n) = next.filter(|n| entry_type(n) == "user") {
                    let result_ids: HashSet<String> = message_content(n)
                        .map(|cs| {
                            cs.iter()
                                .filter(|c| {
                                    c.get("type").and_then(Value::as_str) == Some("tool_result")
                                })
                                .filter_map(|c| {
                                    c.get("tool_use_id").and_then(Value::as_str).map(str::to_owned)
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let all_matched = tool_use_ids.iter().all(|id| result_ids.contains(id));
                    if !all_matched {
                        rules.push("tool-mismatch".into());
                        m += 2;
                        continue;
                    }
                } else {
                    rules.push("dangling-tool-use".into());
                    m += 1;
                    continue;
                }
            }
        }
        validated.push(e.clone());
        m += 1;
    }

    if rules.is_empty() {
        return Ok(SanitizeResult { removed: 0, fixed: 0, rules: vec![] });
    }

    // Pass 4: relink uuid chain across survivors, fix summary.leafUuid.
    let mut summary_idx: Option<usize> = None;
    let mut prev_uuid: Option<String> = None;
    for (idx, e) in validated.iter().enumerate() {
        if entry_type(e) == "summary" {
            summary_idx = Some(idx);
        }
    }
    for e in validated.iter_mut() {
        if entry_type(e) == "summary" {
            continue;
        }
        if e.get("uuid").and_then(Value::as_str).is_some() {
            if e.get("parentUuid").is_some() {
                e["parentUuid"] = match &prev_uuid {
                    Some(u) => Value::String(u.clone()),
                    None => Value::Null,
                };
            }
            if let Some(u) = e.get("uuid").and_then(Value::as_str) {
                prev_uuid = Some(u.to_owned());
            }
        }
    }
    if let Some(idx) = summary_idx {
        validated[idx]["leafUuid"] = match &prev_uuid {
            Some(u) => Value::String(u.clone()),
            None => Value::Null,
        };
    }
    if prev_uuid.is_some() || validated.iter().all(|e| entry_type(e) == "summary") {
        rules.push("relink".into());
    }

    write_entries(path, &validated)?;

    let mut seen = HashSet::new();
    let mut dedup = Vec::new();
    for r in rules.iter() {
        if seen.insert(r.clone()) {
            dedup.push(r.clone());
        }
    }
    Ok(SanitizeResult {
        removed: original.saturating_sub(validated.len()),
        fixed: dedup.len(),
        rules: dedup,
    })
}

// ─── Prune ────────────────────────────────────────────────────────────────

pub struct PruneResult {
    pub trimmed: usize,
    pub stripped: usize,
    pub bytes_before: usize,
    pub bytes_after: usize,
}

pub fn prune_jsonl(path: &Path) -> Result<PruneResult> {
    prune_jsonl_with(path, 4, 300)
}

pub fn prune_jsonl_with(path: &Path, protect_recent: usize, trim_threshold: usize) -> Result<PruneResult> {
    let mut entries = read_entries(path);
    if entries.is_empty() {
        return Ok(PruneResult { trimmed: 0, stripped: 0, bytes_before: 0, bytes_after: 0 });
    }
    let before_bytes: usize = entries
        .iter()
        .map(|e| serde_json::to_string(e).map(|s| s.len()).unwrap_or(0))
        .sum();

    // Find protection boundary — last N user turns.
    let mut protected_idx = entries.len();
    let mut user_count = 0usize;
    for i in (0..entries.len()).rev() {
        if entry_type(&entries[i]) == "user" {
            user_count += 1;
        }
        if user_count >= protect_recent {
            protected_idx = i;
            break;
        }
    }

    let mut trimmed = 0usize;
    let mut stripped = 0usize;

    for i in 0..entries.len() {
        if i >= protected_idx {
            continue;
        }
        let ty = entry_type(&entries[i]).to_string();
        if ty == "assistant" {
            if let Some(cs) = message_content_mut(&mut entries[i]) {
                let before = cs.len();
                cs.retain(|c| c.get("type").and_then(Value::as_str) != Some("thinking"));
                stripped += before - cs.len();
            }
        }
        if ty == "user" {
            if let Some(cs) = message_content_mut(&mut entries[i]) {
                for c in cs.iter_mut() {
                    if c.get("type").and_then(Value::as_str) != Some("tool_result") {
                        continue;
                    }
                    let body = c.get("content").and_then(Value::as_str).unwrap_or("").to_string();
                    if body.len() > trim_threshold {
                        let first = body.split('\n').next().unwrap_or("").to_string();
                        c["content"] = Value::String(format!(
                            "{first}\n(curated: {} chars trimmed)",
                            body.len()
                        ));
                        trimmed += 1;
                    }
                }
            }
        }
    }

    if trimmed == 0 && stripped == 0 {
        return Ok(PruneResult {
            trimmed: 0,
            stripped: 0,
            bytes_before: before_bytes,
            bytes_after: before_bytes,
        });
    }

    write_entries(path, &entries)?;
    let after_bytes: usize = entries
        .iter()
        .map(|e| serde_json::to_string(e).map(|s| s.len()).unwrap_or(0))
        .sum();
    Ok(PruneResult { trimmed, stripped, bytes_before: before_bytes, bytes_after: after_bytes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cwd_hash_basic() {
        assert_eq!(cwd_hash(Path::new("/root/clwnd")), "-root-clwnd");
        assert_eq!(cwd_hash(Path::new("/a_b.c/d")), "-a-b-c-d");
    }

    #[test]
    fn last_uuid_reads_tail() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.jsonl");
        fs::write(
            &p,
            "{\"type\":\"summary\",\"summary\":\"s\",\"leafUuid\":null}\n\
             {\"type\":\"user\",\"uuid\":\"first\",\"parentUuid\":null}\n\
             {\"type\":\"assistant\",\"uuid\":\"second\",\"parentUuid\":\"first\"}\n",
        )
        .unwrap();
        assert_eq!(last_uuid(&p).as_deref(), Some("second"));
    }
}

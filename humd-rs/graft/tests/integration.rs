//! Integration tests for the `graft` crate.
//!
//! These mirror the TypeScript reference in `fs/session.ts`. All filesystem
//! effects are confined to a per-test `TempDir` — `HOME` is repointed at the
//! tempdir before each call that resolves `~/.claude/projects/...`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use serde_json::{json, Value};
use tempfile::TempDir;

use graft::{
    create_claude_session, graft, last_uuid, prune_jsonl, prune_jsonl_with, sanitize_jsonl,
    session_path,
};

// HOME is process-global; cargo runs tests in parallel threads. Every test
// that touches HOME must hold this guard so the env var stays stable for the
// duration of its filesystem work.
fn home_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

// ─── Helpers ──────────────────────────────────────────────────────────────

/// Build a sandboxed `HOME` for one test. The functions under test resolve
/// the Claude base via `$HOME/.claude` — we redirect it so nothing escapes
/// the tempdir.
fn sandbox() -> (TempDir, MutexGuard<'static, ()>) {
    let guard = home_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    std::env::set_var("HOME", dir.path());
    (dir, guard)
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// Copy a fixture into the tempdir so tests never mutate the checked-in file.
fn stage(home: &TempDir, name: &str) -> PathBuf {
    let dst = home.path().join(name);
    fs::copy(fixture(name), &dst).expect("copy fixture");
    dst
}

fn read_lines(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .expect("read jsonl")
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str::<Value>(l).expect("valid json line"))
        .collect()
}

// ─── session_path ─────────────────────────────────────────────────────────

#[test]
fn session_path_uses_dashed_cwd_under_claude_projects() {
    let (home, _guard) = sandbox();
    let cwd = Path::new("/tmp/proj");
    let sid = "ses_abc123";

    let p = session_path(cwd, sid);

    let expected = home
        .path()
        .join(".claude")
        .join("projects")
        .join("-tmp-proj")
        .join(format!("{sid}.jsonl"));
    assert_eq!(p, expected);
}

#[test]
fn session_path_strips_trailing_dashes_only() {
    let (_home, _guard) = sandbox();
    // Slashes become dashes; interior runs are NOT collapsed — only
    // trailing dashes are trimmed.
    let p = session_path(Path::new("/a//b/"), "sid");
    let s = p.to_string_lossy().into_owned();
    assert!(s.contains("-a--b/"), "got {s}");
    assert!(s.ends_with("/sid.jsonl"), "got {s}");
    assert!(!s.contains("-a--b-/"), "trailing dash must be stripped: {s}");
}

// ─── create_claude_session ────────────────────────────────────────────────

#[test]
fn create_claude_session_creates_parent_dir_and_seed_jsonl() {
    let (_home, _guard) = sandbox();
    let cwd = Path::new("/tmp/proj");
    let sid = "ses_seed";

    let path = create_claude_session(cwd, sid).expect("create");

    assert!(path.exists(), "jsonl file should exist");
    assert!(path.parent().unwrap().is_dir(), "parent dir should exist");

    let lines = read_lines(&path);
    assert_eq!(lines.len(), 1, "seed jsonl has exactly one summary line");
    assert_eq!(lines[0]["type"], "summary");
    assert_eq!(lines[0]["sessionId"], sid);
    assert!(lines[0]["leafUuid"].is_null());
}

// ─── graft ────────────────────────────────────────────────────────────────

#[test]
fn graft_on_empty_priors_returns_zero() {
    let (home, _guard) = sandbox();
    let jsonl = stage(&home, "empty.jsonl");

    let result = graft(&[], &jsonl, "ses_test_0001", Path::new("/tmp/proj"), None)
        .expect("graft ok");

    assert_eq!(result.grafted, 0);
    // Body unchanged: still just the summary line.
    let lines = read_lines(&jsonl);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["type"], "summary");
}

#[test]
fn graft_appends_new_entries_with_valid_parent_chain() {
    let (home, _guard) = sandbox();
    let jsonl = stage(&home, "empty.jsonl");

    // Two completed turns: u -> a, u -> a. Trailing user prompt is the
    // "live" message that murmur owns and must NOT be grafted.
    let priors = vec![
        json!({"role": "user", "content": "first prompt"}),
        json!({"role": "assistant", "content": "first reply"}),
        json!({"role": "user", "content": "second prompt"}),
        json!({"role": "assistant", "content": "second reply"}),
        json!({"role": "user", "content": "live prompt (not grafted)"}),
    ];

    let result = graft(&priors, &jsonl, "ses_test_0001", Path::new("/tmp/proj"), None)
        .expect("graft ok");

    assert_eq!(result.grafted, 2, "two assistant turns grafted");

    let lines = read_lines(&jsonl);
    // 1 summary + 4 conversation entries (2 user, 2 assistant).
    assert_eq!(lines.len(), 5, "lines: {lines:#?}");

    // Walk parent chain across the conversation lines: each line's
    // parentUuid must be the previous line's uuid, starting from null.
    let mut prev: Option<String> = None;
    for line in lines.iter().skip(1) {
        let uuid = line["uuid"].as_str().expect("uuid").to_string();
        // uuid v4: 8-4-4-4-12 hex
        assert_eq!(uuid.len(), 36, "uuid must be 36 chars, got {uuid:?}");
        assert_eq!(uuid.matches('-').count(), 4, "uuid must have 4 dashes");

        let parent = line["parentUuid"].as_str().map(String::from);
        assert_eq!(parent, prev, "parentUuid must link to previous entry");
        prev = Some(uuid);
    }

    // `last_petal` reflects the last grafted uuid (= last line's uuid).
    assert_eq!(result.last_petal.as_deref(), prev.as_deref());
}

// ─── sanitize_jsonl ───────────────────────────────────────────────────────

#[test]
fn sanitize_strips_trailing_partial_tool_use() {
    let (home, _guard) = sandbox();
    let jsonl = stage(&home, "partial_tool_use.jsonl");
    let before = read_lines(&jsonl);
    // sanity: fixture ends with a tool_use assistant with no following tool_result.
    let last = before.last().unwrap();
    assert_eq!(last["type"], "assistant");

    let result = sanitize_jsonl(&jsonl).expect("sanitize");

    assert!(result.removed >= 1, "must remove at least the dangling assistant");
    assert!(
        result
            .rules
            .iter()
            .any(|r| r == "trailing-tool-use" || r == "dangling-tool-use"),
        "rules should mention the trailing-tool-use rule, got {:?}",
        result.rules,
    );

    let after = read_lines(&jsonl);
    let any_dangling_tool_use = after.iter().any(|e| {
        e["type"] == "assistant"
            && e["message"]["content"]
                .as_array()
                .map(|arr| arr.iter().any(|c| c["type"] == "tool_use"))
                .unwrap_or(false)
    });
    assert!(!any_dangling_tool_use, "no dangling tool_use should remain");
}

// ─── last_uuid ────────────────────────────────────────────────────────────

#[test]
fn last_uuid_returns_final_entry_uuid() {
    let (home, _guard) = sandbox();
    let jsonl = stage(&home, "two_turns.jsonl");
    let u = last_uuid(&jsonl);
    assert_eq!(
        u.as_deref(),
        Some("11111111-2222-3333-4444-000000000004"),
        "should return uuid of last line",
    );
}

#[test]
fn last_uuid_returns_none_when_no_uuids() {
    let (home, _guard) = sandbox();
    // empty.jsonl has only a summary line (no `uuid` field).
    let jsonl = stage(&home, "empty.jsonl");
    let u = last_uuid(&jsonl);
    assert_eq!(u, None);
}

#[test]
fn last_uuid_returns_none_for_missing_file() {
    let (home, _guard) = sandbox();
    let missing = home.path().join("nope.jsonl");
    assert_eq!(last_uuid(&missing), None);
}

// ─── prune_jsonl ──────────────────────────────────────────────────────────

#[test]
fn prune_shrinks_bytes_when_thinking_blocks_present() {
    let (home, _guard) = sandbox();
    let jsonl = stage(&home, "with_thinking.jsonl");
    let before_bytes = fs::metadata(&jsonl).unwrap().len();

    // protect_recent=1 keeps only the *last* user turn fully intact, which
    // means both of the assistant turns carrying thinking blocks fall
    // outside the protection window. With the default protect_recent=4
    // and only 4 user turns, every entry is protected — no stripping.
    let result = prune_jsonl_with(&jsonl, 1, 300).expect("prune");

    assert!(
        result.stripped >= 1,
        "at least one thinking block should be stripped (stripped={}, trimmed={})",
        result.stripped,
        result.trimmed,
    );
    assert!(
        result.bytes_after < result.bytes_before,
        "byte count must shrink: before={} after={}",
        result.bytes_before,
        result.bytes_after,
    );

    let after_bytes = fs::metadata(&jsonl).unwrap().len();
    assert!(after_bytes < before_bytes, "on-disk file must shrink");
}

#[test]
fn prune_default_no_change_when_only_recent_thinking() {
    // The fixture only has 4 user turns; with default protect_recent=4
    // every turn is protected and nothing is stripped. This documents
    // the protection invariant so we don't accidentally regress on it.
    let (home, _guard) = sandbox();
    let jsonl = stage(&home, "with_thinking.jsonl");
    let before_bytes = fs::metadata(&jsonl).unwrap().len();

    let result = prune_jsonl(&jsonl).expect("prune");

    assert_eq!(result.stripped, 0);
    assert_eq!(result.trimmed, 0);
    let after_bytes = fs::metadata(&jsonl).unwrap().len();
    assert_eq!(after_bytes, before_bytes, "file must be untouched");
}

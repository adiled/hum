//! Glob — file discovery by pattern, skipping the usual junk dirs.

use crate::protocol::{ToolDef, ToolResult};
use crate::session::SessionState;
use crate::tools::fs_util::assert_path;
use parking_lot::Mutex;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_RESULTS: usize = 200;
const MAX_DEPTH: usize = 30;

const SKIP_DIRS: &[&str] = &[
    "node_modules", ".git", ".svn", ".hg", "dist", "build", ".next", ".nuxt", ".turbo", "target",
    "__pycache__", ".venv", "venv", ".mypy_cache", ".pytest_cache", ".cache", "coverage", ".idea",
    ".vscode",
];

#[derive(Deserialize)]
struct Args {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
}

pub fn def() -> ToolDef {
    ToolDef {
        name: "Glob".to_string(),
        description: "Find files matching a glob pattern. Supports `**` for any-depth descent. Walks from `path` (default: session cwd). Sorted newest-first; capped at 200 results.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "e.g. '**/*.ts' or 'src/**/handlers/*.rs'." },
                "path":    { "type": "string", "description": "Base directory. Defaults to session cwd." },
            },
            "required": ["pattern"],
        }),
    }
}

pub fn run(args: Value, session: &Arc<Mutex<SessionState>>) -> ToolResult {
    let args: Args = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return ToolResult::error(format!("invalid args: {e}")),
    };
    let snap = session.lock().clone();
    let base = match args.path.as_deref() {
        Some(p) => match assert_path(p, &snap) { Ok(p) => p, Err(e) => return ToolResult::error(e) },
        None => snap.cwd.clone(),
    };
    if let Err(e) = snap.check_permission("Glob", base.to_str()) {
        return ToolResult::error(e);
    }

    let segs: Vec<&str> = args.pattern.split('/').filter(|s| !s.is_empty()).collect();
    let mut hits: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
    walk(&base, &segs, 0, &mut hits);
    hits.sort_by(|a, b| b.1.cmp(&a.1));

    if hits.is_empty() {
        return ToolResult::text(format!("(no files matched {})", args.pattern));
    }
    let mut out = String::new();
    for (p, _) in hits.iter().take(MAX_RESULTS) {
        out.push_str(&p.display().to_string());
        out.push('\n');
    }
    if hits.len() > MAX_RESULTS {
        out.push_str(&format!("[... {} more, truncated]\n", hits.len() - MAX_RESULTS));
    }
    ToolResult::text(out)
}

fn walk(dir: &Path, pat: &[&str], depth: usize, out: &mut Vec<(PathBuf, std::time::SystemTime)>) {
    if depth > MAX_DEPTH || out.len() >= MAX_RESULTS {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for ent in entries.flatten() {
        if out.len() >= MAX_RESULTS { return; }
        let name = ent.file_name();
        let name_s = name.to_string_lossy();
        let full = ent.path();
        let is_dir = ent.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            if SKIP_DIRS.contains(&name_s.as_ref()) || name_s.starts_with('.') {
                continue;
            }
            walk(&full, pat, depth + 1, out);
        } else if ent.file_type().map(|t| t.is_file()).unwrap_or(false) {
            // match the path segments below `dir`.
            let rel: Vec<String> = full
                .strip_prefix(dir)
                .ok()
                .map(|r| r.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect())
                .unwrap_or_default();
            if matches_pattern(&rel, pat) {
                if let Ok(meta) = ent.metadata() {
                    let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                    out.push((full, mtime));
                }
            }
        }
    }
}

fn matches_pattern(path: &[String], pat: &[&str]) -> bool {
    go(path, 0, pat, 0)
}

fn go(path: &[String], i: usize, pat: &[&str], j: usize) -> bool {
    if j == pat.len() { return i == path.len(); }
    if pat[j] == "**" {
        for k in i..=path.len() {
            if go(path, k, pat, j + 1) {
                return true;
            }
        }
        return false;
    }
    if i == path.len() { return false; }
    if !seg_match(pat[j], &path[i]) { return false; }
    go(path, i + 1, pat, j + 1)
}

fn seg_match(pat: &str, name: &str) -> bool {
    // Translate one segment to regex: * -> [^/]*, ? -> [^/].
    let mut re = String::with_capacity(pat.len() * 2);
    re.push('^');
    for c in pat.chars() {
        match c {
            '*' => re.push_str("[^/]*"),
            '?' => re.push_str("[^/]"),
            '.' | '+' | '(' | ')' | '|' | '[' | ']' | '\\' | '^' | '$' => {
                re.push('\\');
                re.push(c);
            }
            other => re.push(other),
        }
    }
    re.push('$');
    Regex::new(&re).map(|r| r.is_match(name)).unwrap_or(false)
}

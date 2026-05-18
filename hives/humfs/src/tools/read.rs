//! `humfs_read` — the ONE filesystem analysis tool.
//!
//! P2 covers the no-AST mode:
//!
//! - **Path resolution**: file | directory | glob (auto-detected by
//!   `*` or `?` in the path).
//! - **Modifier-free single-target**: line-numbered preamble +
//!   stats; AST symbol outline is a stub until P3/P4.
//! - **Modifier-free multi-target**: inventory view (one line per
//!   resolved file, line count + size).
//! - **`pattern`**: regex over file CONTENT. Returns matching lines
//!   with `path:line` annotation. Code-file enclosing-symbol
//!   annotation arrives with P4 (AST infra).
//! - **`symbol` / `query`**: stubs returning "lands in P4".
//!
//! Skips junk dirs (`node_modules`, `.git`, `target`, `__pycache__`,
//! `dist`, `build`, etc.) on dir walks and glob expansion. Caps
//! resolved targets at 200 (a `read('/')` doesn't explode). Caps
//! output at 7500 chars (safely under Claude CLI's per-tool-result
//! ceiling).

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use base64::Engine;
use nest_common::{ToolDef, ToolResult};
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::ast;

const MAX_RESOLVED_TARGETS: usize = 200;
const MAX_READ_OUTPUT: usize = 7500;
const MAX_READ_BYTES: usize = 256 * 1024;
const MAX_DEPTH: usize = 30;
const STUDY_PREAMBLE_LINES: usize = 20;

fn skip_dirs() -> HashSet<&'static str> {
    [
        "node_modules", ".git", ".svn", ".hg", "dist", "build",
        ".next", ".nuxt", ".turbo", "target", "__pycache__",
        ".venv", "venv", ".mypy_cache", ".pytest_cache", ".cache",
        "coverage", ".idea", ".vscode",
    ].into_iter().collect()
}

#[derive(Deserialize, Default)]
struct Args {
    file_path: String,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    pattern: Option<String>,
}

pub fn def() -> ToolDef {
    ToolDef {
        name: "humfs_read".into(),
        description: "Filesystem analysis: discover, study, and search. Works on any file — code returns a tree-sitter symbol outline (P4+); configs and docs return an anchor outline; extensionless files (Dockerfile, Makefile, LICENSE) and unknown extensions return content. Path auto-detection: file | directory | glob (presence of * or ?). Pick at most one modifier: symbol (exact, dot-nested for nested members), query (fuzzy case-insensitive substring match on symbol NAMES), pattern (regex over CONTENT — code matches carry their enclosing function/class symbol in P4+). The tool decides framing; no offset, no limit, no pagination.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "Absolute file path, absolute directory path, or glob pattern (detected by presence of * or ?)." },
                "symbol":    { "type": "string", "description": "Extract a specific symbol by exact name. Dot-separated for nested (e.g. 'Class.method')." },
                "query":     { "type": "string", "description": "Fuzzy case-insensitive substring match on symbol NAMES." },
                "pattern":   { "type": "string", "description": "Regex over file CONTENT. For code, each match is annotated with its enclosing function/class symbol." },
            },
            "required": ["file_path"],
        }),
    }
}

pub async fn run(args: Value) -> ToolResult {
    let args: Args = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return ToolResult::error(format!("invalid args: {e}")),
    };

    if args.file_path.is_empty() {
        return ToolResult::error("file_path is required");
    }

    let targets = resolve_targets(&args.file_path);
    if targets.is_empty() {
        return ToolResult::error(format!(
            "No files resolved from '{}'. Check the path — it can be an absolute file, an absolute directory, or a glob pattern (e.g. '/src/**/*.ts').",
            args.file_path
        ));
    }

    if let Some(sym) = args.symbol.as_deref() {
        return read_by_symbol(&targets, sym);
    }
    if let Some(q) = args.query.as_deref() {
        return read_by_query(&targets, q);
    }
    if let Some(pat) = args.pattern.as_deref() {
        return read_by_pattern(&targets, pat);
    }

    if targets.len() == 1 {
        return study_single(&targets[0]);
    }
    inventory(&targets)
}

// ── path resolution ──────────────────────────────────────────────────────

fn is_glob(path: &str) -> bool {
    let glob_chars = Regex::new(r"[*?\[\]]").unwrap();
    glob_chars.is_match(path)
}

fn resolve_targets(raw_path: &str) -> Vec<PathBuf> {
    if is_glob(raw_path) {
        return expand_glob(raw_path);
    }
    let p = PathBuf::from(raw_path);
    let meta = match fs::metadata(&p) {
        Ok(m) => m,
        Err(_) => return vec![],
    };
    if meta.is_file() {
        return vec![p];
    }
    if meta.is_dir() {
        return walk_dir(&p, MAX_RESOLVED_TARGETS);
    }
    vec![]
}

fn walk_dir(dir: &Path, max: usize) -> Vec<PathBuf> {
    let skip = skip_dirs();
    let mut results = Vec::new();
    let mut stack: Vec<PathBuf> = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if results.len() >= max { break; }
        let entries = match fs::read_dir(&d) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if results.len() >= max { break; }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let ft = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ft.is_dir() {
                if skip.contains(name.as_str()) || name.starts_with('.') { continue; }
                stack.push(path);
            } else if ft.is_file() {
                results.push(path);
            }
        }
    }
    results
}

fn seg_to_regex(seg: &str) -> Regex {
    let mut esc = String::new();
    for ch in seg.chars() {
        match ch {
            '.' | '+' | '^' | '$' | '(' | ')' | '|' | '[' | ']' | '\\' => {
                esc.push('\\');
                esc.push(ch);
            }
            '*' => esc.push_str("[^/]*"),
            '?' => esc.push_str("[^/]"),
            _ => esc.push(ch),
        }
    }
    Regex::new(&format!("^{esc}$")).expect("glob seg regex")
}

fn expand_glob(pattern: &str) -> Vec<PathBuf> {
    let (base_dir, pat) = if let Some(stripped) = pattern.strip_prefix('/') {
        let parts: Vec<&str> = stripped.split('/').collect();
        let first_wild = parts.iter().position(|seg| is_glob(seg));
        match first_wild {
            None => {
                let p = PathBuf::from(pattern);
                return if p.exists() { vec![p] } else { vec![] };
            }
            Some(idx) => {
                let base = if idx == 0 { "/".to_string() } else { format!("/{}", parts[..idx].join("/")) };
                let pat = parts[idx..].join("/");
                (PathBuf::from(base), pat)
            }
        }
    } else {
        // Relative — anchor at cwd. Process cwd at boot time; OK for v0.
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        (cwd, pattern.to_string())
    };

    let segs: Vec<String> = pat.split('/').filter(|s| !s.is_empty()).map(str::to_string).collect();
    let mut results: Vec<(PathBuf, SystemTime)> = Vec::new();
    let skip = skip_dirs();

    fn matches(segs: &[String], path_segs: &[String]) -> bool {
        fn go(segs: &[String], path_segs: &[String], i: usize, j: usize) -> bool {
            if j == segs.len() { return i == path_segs.len(); }
            if segs[j] == "**" {
                for k in i..=path_segs.len() {
                    if go(segs, path_segs, k, j + 1) { return true; }
                }
                return false;
            }
            if i == path_segs.len() { return false; }
            if !seg_to_regex(&segs[j]).is_match(&path_segs[i]) { return false; }
            go(segs, path_segs, i + 1, j + 1)
        }
        go(segs, path_segs, 0, 0)
    }

    fn walk(
        dir: &Path,
        depth: usize,
        base_dir: &Path,
        segs: &[String],
        skip: &HashSet<&'static str>,
        results: &mut Vec<(PathBuf, SystemTime)>,
    ) {
        if results.len() >= MAX_RESOLVED_TARGETS || depth > MAX_DEPTH { return; }
        let entries = match fs::read_dir(dir) { Ok(e) => e, Err(_) => return };
        for entry in entries.flatten() {
            if results.len() >= MAX_RESOLVED_TARGETS { return; }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let ft = match entry.file_type() { Ok(t) => t, Err(_) => continue };
            if ft.is_dir() {
                if skip.contains(name.as_str()) || name.starts_with('.') { continue; }
                walk(&path, depth + 1, base_dir, segs, skip, results);
            } else if ft.is_file() {
                let rel = path.strip_prefix(base_dir).unwrap_or(&path);
                let rel_segs: Vec<String> = rel
                    .components()
                    .filter_map(|c| c.as_os_str().to_str().map(String::from))
                    .collect();
                if matches(segs, &rel_segs) {
                    let mtime = fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .unwrap_or(SystemTime::UNIX_EPOCH);
                    results.push((path, mtime));
                }
            }
        }
    }

    walk(&base_dir, 0, &base_dir, &segs, &skip, &mut results);
    results.sort_by(|a, b| b.1.cmp(&a.1));
    results.into_iter().map(|(p, _)| p).collect()
}

// ── modifier-free views ──────────────────────────────────────────────────

fn study_single(path: &Path) -> ToolResult {
    if is_image(path) { return study_image(path); }

    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) => return ToolResult::error(format!("read failed: {e}")),
    };
    let trunc = bytes.len() > MAX_READ_BYTES;
    let slice = if trunc { &bytes[..MAX_READ_BYTES] } else { &bytes[..] };
    let content = String::from_utf8_lossy(slice);
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let preamble_n = STUDY_PREAMBLE_LINES.min(total);
    let size = bytes.len();

    let mut out = String::new();
    out.push_str(&format!("=== {} ===\n", path.display()));
    out.push_str(&format!("[{size} bytes, {total} lines]\n"));

    // AST-aware section: outline first (so callers know what
    // symbols to drill into), then the preamble lines for context.
    let outline = ast::detect_language(path).map(|lang| {
        let syms = ast::file_symbols(&content, lang);
        (lang, syms)
    });
    if let Some((lang, syms)) = &outline {
        out.push_str(&format!("[{} — {} symbol(s)]\n\n", lang.name(), syms.len()));
        out.push_str("outline:\n");
        out.push_str(&ast::outline::format_symbols(syms));
        out.push('\n');
    }

    out.push_str("preamble:\n");
    for (i, line) in lines.iter().take(preamble_n).enumerate() {
        out.push_str(&format!("{:>6}\t{line}\n", i + 1));
    }
    if total > preamble_n {
        out.push_str(&format!(
            "…\n[{} more lines — pass symbol='Name' for a specific symbol, pattern='regex' for content search, or query='sub' for a fuzzy name match]\n",
            total - preamble_n
        ));
    }
    if trunc {
        out.push_str(&format!("[humfs: truncated at {} KB — file is larger]\n", MAX_READ_BYTES / 1024));
    }
    cap_output(out, Some(path))
}

fn inventory(targets: &[PathBuf]) -> ToolResult {
    let mut out = String::new();
    out.push_str(&format!("=== {} files ===\n", targets.len()));
    for t in targets {
        let size = fs::metadata(t).map(|m| m.len()).unwrap_or(0);
        let lines = safe_line_count(t);
        out.push_str(&format!("{:>10}b  {:>6}L  {}\n", size, lines, t.display()));
    }
    out.push_str("\n[Inventory view — pass pattern='regex' to grep content, or pick a single file for a study view.]\n");
    cap_output(out, None)
}

// ── pattern search ───────────────────────────────────────────────────────

fn read_by_pattern(targets: &[PathBuf], pattern: &str) -> ToolResult {
    let re = match Regex::new(pattern) {
        Ok(r) => r,
        Err(e) => return ToolResult::error(format!("invalid regex: {e}")),
    };
    let mut out = String::new();
    let mut hits = 0usize;
    let mut files_with_hits = 0usize;
    for path in targets {
        let content = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        // For code files, build the symbol index once so each hit
        // can carry the enclosing function/class name.
        let lang_syms = ast::detect_language(path)
            .map(|lang| ast::file_symbols(&content, lang));
        let mut file_hits = 0usize;
        let mut byte_cursor = 0usize;
        for (lineno, line) in content.lines().enumerate() {
            if re.is_match(line) {
                hits += 1;
                file_hits += 1;
                let enclosing = lang_syms.as_ref()
                    .and_then(|syms| ast::enclosing_symbol(syms, byte_cursor));
                if let Some(sym) = enclosing {
                    out.push_str(&format!(
                        "{}:{} [{} {}]\t{}\n",
                        path.display(), lineno + 1, sym.kind.tag(), sym.name, line
                    ));
                } else {
                    out.push_str(&format!("{}:{}\t{}\n", path.display(), lineno + 1, line));
                }
                if out.len() > MAX_READ_OUTPUT { break; }
            }
            byte_cursor += line.len() + 1; // +1 for the '\n'
        }
        if file_hits > 0 { files_with_hits += 1; }
        if out.len() > MAX_READ_OUTPUT { break; }
    }
    if hits == 0 {
        return ToolResult {
            output: format!("[no matches for /{pattern}/ across {} target(s)]", targets.len()),
            title: Some(format!("pattern: {pattern}")),
            metadata: Some(json!({ "hits": 0, "targets": targets.len() })),
            is_error: false,
        };
    }
    let mut header = format!("[{hits} match(es) across {files_with_hits} file(s)]\n\n");
    header.push_str(&out);
    cap_output(header, Some(&PathBuf::from(format!("pattern:{pattern}"))))
}

// ── symbol / query ──────────────────────────────────────────────────────

/// Find a symbol by path. Supports plain names, dot-nested
/// ("Class.method"), and sub-symbol alias walks ("alpha.body",
/// "alpha.when.otherwise", "alpha.loop#2.body").
fn read_by_symbol(targets: &[PathBuf], symbol: &str) -> ToolResult {
    let mut out = String::new();
    let mut matches = 0usize;
    for path in targets {
        let lang = match ast::detect_language(path) { Some(l) => l, None => continue };
        let content = match fs::read_to_string(path) { Ok(s) => s, Err(_) => continue };
        if let Some((start, end, start_row, end_row)) = ast::resolve_path(&content, lang, symbol) {
            matches += 1;
            out.push_str(&format!("=== {} — '{symbol}' (L{start_row}-L{end_row}) ===\n",
                path.display()));
            let slice = content.get(start..end).unwrap_or("");
            for (i, line) in slice.lines().enumerate() {
                out.push_str(&format!("{:>6}\t{line}\n", start_row + i));
            }
            out.push('\n');
            if out.len() > MAX_READ_OUTPUT { break; }
        }
    }
    if matches == 0 {
        return ToolResult::error(format!(
            "no symbol path '{symbol}' across {} target(s)", targets.len()
        ));
    }
    cap_output(out, Some(&PathBuf::from(format!("symbol:{symbol}"))))
}

/// Fuzzy case-insensitive substring match on symbol names. Returns
/// each matched symbol's source, one block per match.
fn read_by_query(targets: &[PathBuf], query: &str) -> ToolResult {
    let needle = query.to_lowercase();
    let mut out = String::new();
    let mut matches = 0usize;
    for path in targets {
        let lang = match ast::detect_language(path) { Some(l) => l, None => continue };
        let content = match fs::read_to_string(path) { Ok(s) => s, Err(_) => continue };
        let syms = ast::file_symbols(&content, lang);
        for sym in syms.iter().filter(|s| s.name.to_lowercase().contains(&needle)) {
            matches += 1;
            out.push_str(&format!("=== {} — {} {} (L{}-L{}) ===\n",
                path.display(), sym.kind.tag(), sym.name, sym.start_row, sym.end_row));
            let slice = content.get(sym.start_byte..sym.end_byte).unwrap_or("");
            for (i, line) in slice.lines().enumerate() {
                out.push_str(&format!("{:>6}\t{line}\n", sym.start_row + i));
            }
            out.push('\n');
            if out.len() > MAX_READ_OUTPUT { break; }
        }
        if out.len() > MAX_READ_OUTPUT { break; }
    }
    if matches == 0 {
        return ToolResult::error(format!(
            "no symbol name matches '{query}' across {} target(s)", targets.len()
        ));
    }
    cap_output(out, Some(&PathBuf::from(format!("query:{query}"))))
}

// ── images ──────────────────────────────────────────────────────────────

fn is_image(path: &Path) -> bool {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "ico" | "svg")
}

fn study_image(path: &Path) -> ToolResult {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) => return ToolResult::error(format!("read failed: {e}")),
    };
    let mime = mime_guess::from_path(path).first_or_octet_stream().to_string();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    ToolResult {
        output: format!("[image:{mime}, {} bytes]\n{b64}", bytes.len()),
        title: Some(format!("image: {}", path.display())),
        metadata: Some(json!({ "mime": mime, "bytes": bytes.len() })),
        is_error: false,
    }
}

// ── code file detection ─────────────────────────────────────────────────

fn is_code_file(path: &Path) -> bool {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs"
        | "py" | "pyi"
        | "go"
        | "rs"
        | "java"
        | "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" | "hxx"
        | "rb"
        | "php"
        | "cs"
        | "kt" | "kts"
        | "swift"
        | "scala"
        | "lua"
        | "sh" | "bash" | "zsh" | "fish"
        | "vue" | "svelte"
        | "sql"
    )
}

// ── helpers ─────────────────────────────────────────────────────────────

fn safe_line_count(p: &Path) -> usize {
    fs::read_to_string(p).map(|s| s.lines().count()).unwrap_or(0)
}

fn cap_output(mut s: String, title_path: Option<&Path>) -> ToolResult {
    let truncated = s.len() > MAX_READ_OUTPUT;
    if truncated {
        // Truncate on a char boundary near MAX_READ_OUTPUT.
        let mut idx = MAX_READ_OUTPUT;
        while !s.is_char_boundary(idx) { idx -= 1; }
        s.truncate(idx);
        s.push_str(&format!("\n[humfs: output truncated at {MAX_READ_OUTPUT} chars]\n"));
    }
    ToolResult {
        output: s,
        title: title_path.map(|p| p.display().to_string()),
        metadata: Some(json!({ "truncated": truncated })),
        is_error: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_glob_detects_star_question() {
        assert!(is_glob("/tmp/**/*.rs"));
        assert!(is_glob("foo?.txt"));
        assert!(!is_glob("/tmp/plain.rs"));
    }

    #[test]
    fn seg_regex_handles_specials() {
        let r = seg_to_regex("*.rs");
        assert!(r.is_match("foo.rs"));
        assert!(!r.is_match("foo.txt"));
        let r2 = seg_to_regex("test_?.py");
        assert!(r2.is_match("test_1.py"));
        assert!(!r2.is_match("test_xx.py"));
    }

    #[test]
    fn is_code_file_known_exts() {
        assert!(is_code_file(Path::new("/x/y.ts")));
        assert!(is_code_file(Path::new("/x/y.rs")));
        assert!(is_code_file(Path::new("/x/y.py")));
        assert!(!is_code_file(Path::new("/x/y.md")));
        assert!(!is_code_file(Path::new("/x/y.json")));
    }
}

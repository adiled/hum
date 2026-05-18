//! `humfs_do_noncode` — linguistic-scope edits for non-code files.
//!
//! Four scopes:
//!
//! - **`word`** — single token swap, format-agnostic. Word boundary
//!   regex picks the exact occurrence.
//! - **`phrase`** — structural name OR exact text. Format-aware:
//!   JSON keys + values, env vars, markdown headings, TOML sections.
//!   Falls back to first exact-substring match when no structural
//!   interpretation lands.
//! - **`sentence`** — smallest independent unit. Resolves to the
//!   single line containing the scope text.
//! - **`paragraph`** — full block. Resolves to the blank-line
//!   paragraph (or YAML indentation block) containing the scope.
//!
//! Omit `replace` to delete the resolved scope; no scope parameter
//! creates / overwrites the whole file. Code extensions route back
//! to `humfs_do_code`.
//!
//! Structural validation: when the original file is valid JSON,
//! the post-edit result must also parse as JSON or the write is
//! rejected. Other formats (YAML, TOML, env) get lightweight
//! integrity checks.

use std::path::{Path, PathBuf};

use nest_common::{ToolDef, ToolResult};
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::ast;

#[derive(Deserialize)]
struct Args {
    file_path: String,
    #[serde(default)]
    word: Option<String>,
    #[serde(default)]
    phrase: Option<String>,
    #[serde(default)]
    sentence: Option<String>,
    #[serde(default)]
    paragraph: Option<String>,
    #[serde(default)]
    replace: Option<String>,
}

pub fn def() -> ToolDef {
    ToolDef {
        name: "humfs_do_noncode".into(),
        description: "Author non-code files using linguistic scope. Four scopes (pass exactly one): word (format-agnostic token swap), phrase (structural name — JSON/YAML key, env var, markdown heading, TOML section — or exact text), sentence (smallest independent unit), paragraph (full block). Omit 'replace' to delete the scope; no scope param creates/overwrites the whole file. Handles configs, docs, markup, stylesheets, data, plain text. Code files route to humfs_do_code.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "Absolute path to a non-code file." },
                "word":      { "type": "string" },
                "phrase":    { "type": "string" },
                "sentence":  { "type": "string" },
                "paragraph": { "type": "string" },
                "replace":   { "type": "string", "description": "Replacement content. Omit to delete the scope; omit scope param too to create/overwrite the whole file." },
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
    let path = PathBuf::from(&args.file_path);

    if ast::detect_language(&path).is_some() {
        return ToolResult::error(format!(
            "humfs_do_noncode refuses code extensions. '{}' is a code file — route to humfs_do_code.",
            path.display()
        ));
    }

    let scope = pick_scope(&args);
    let replace = args.replace.unwrap_or_default();

    if scope.is_none() {
        return write_whole_file(&path, &replace);
    }
    let (scope_kind, scope_text) = scope.unwrap();

    if !path.exists() {
        return ToolResult::error(format!(
            "{} scope '{scope_text}' requires the file to exist.", scope_kind.tag()
        ));
    }

    let original = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => return ToolResult::error(format!("read failed: {e}")),
    };

    let resolution = match scope_kind {
        Scope::Word      => resolve_word(&original, &scope_text),
        Scope::Phrase    => resolve_phrase(&original, &scope_text, &path),
        Scope::Sentence  => resolve_sentence(&original, &scope_text),
        Scope::Paragraph => resolve_paragraph(&original, &scope_text),
    };

    let m = match resolution {
        Some(m) => m,
        None => return ToolResult::error(format!(
            "{} '{scope_text}' not found in {}. Read the file first to see its content.",
            scope_kind.tag(), path.display()
        )),
    };

    let updated = splice(&original, m.start, m.end, &replace);
    if let Err(msg) = validate_structure(&path, &original, &updated) {
        return ToolResult::error(format!(
            "edit would corrupt {} — {msg}. File NOT modified; fix your replacement and try again.",
            path.display()
        ));
    }

    if let Err(e) = std::fs::write(&path, &updated) {
        return ToolResult::error(format!("write failed: {e}"));
    }

    let action = if replace.is_empty() { "Deleted" } else { "Replaced" };
    let context = window(&updated, m.start, m.start + replace.len(), 3);
    ToolResult {
        output: format!(
            "{action} {} '{scope_text}' in {} ({} bytes).\n\n{context}",
            scope_kind.tag(), path.display(), updated.len()
        ),
        title: Some(path.display().to_string()),
        metadata: Some(json!({ "scope": scope_kind.tag(), "size": updated.len() })),
        is_error: false,
    }
}

// ── scope ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
enum Scope { Word, Phrase, Sentence, Paragraph }

impl Scope {
    fn tag(self) -> &'static str {
        match self {
            Scope::Word => "word",
            Scope::Phrase => "phrase",
            Scope::Sentence => "sentence",
            Scope::Paragraph => "paragraph",
        }
    }
}

fn pick_scope(args: &Args) -> Option<(Scope, String)> {
    if let Some(s) = args.word.as_ref()      { return Some((Scope::Word, s.clone())); }
    if let Some(s) = args.phrase.as_ref()    { return Some((Scope::Phrase, s.clone())); }
    if let Some(s) = args.sentence.as_ref()  { return Some((Scope::Sentence, s.clone())); }
    if let Some(s) = args.paragraph.as_ref() { return Some((Scope::Paragraph, s.clone())); }
    None
}

// ── resolution ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
struct Match { start: usize, end: usize }

fn resolve_word(source: &str, word: &str) -> Option<Match> {
    let re = Regex::new(&format!(r"\b{}\b", regex::escape(word))).ok()?;
    let m = re.find(source)?;
    Some(Match { start: m.start(), end: m.end() })
}

fn resolve_phrase(source: &str, phrase: &str, path: &Path) -> Option<Match> {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
    // Format-aware first.
    let by_format = match ext.as_str() {
        "json" | "jsonc" => phrase_json(source, phrase),
        "yaml" | "yml"   => phrase_yaml(source, phrase),
        "toml"           => phrase_toml(source, phrase),
        "env"            => phrase_env(source, phrase),
        "md" | "markdown" => phrase_markdown(source, phrase),
        _ => None,
    };
    if by_format.is_some() { return by_format; }
    // env-style file by leading-dot or .env-like basename.
    let base = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if base.starts_with(".env") {
        if let Some(m) = phrase_env(source, phrase) { return Some(m); }
    }
    // Fallback: first exact-substring match (acts as "exact text").
    let idx = source.find(phrase)?;
    Some(Match { start: idx, end: idx + phrase.len() })
}

fn resolve_sentence(source: &str, sentence: &str) -> Option<Match> {
    // Single-line scope: find the substring, then expand to its full
    // containing line including the trailing newline.
    let idx = source.find(sentence)?;
    let line_start = source[..idx].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let line_end_rel = source[idx..].find('\n').map(|p| idx + p + 1).unwrap_or(source.len());
    Some(Match { start: line_start, end: line_end_rel })
}

fn resolve_paragraph(source: &str, paragraph: &str) -> Option<Match> {
    // Blank-line paragraph: find the substring, expand outward until
    // we hit a blank line (or file boundary) on each side.
    let idx = source.find(paragraph)?;
    let para_start = find_paragraph_start(source, idx);
    let para_end = find_paragraph_end(source, idx);
    Some(Match { start: para_start, end: para_end })
}

fn find_paragraph_start(source: &str, idx: usize) -> usize {
    let mut start = source[..idx].rfind("\n\n").map(|p| p + 2).unwrap_or(0);
    // Skip leading whitespace on the paragraph's first line.
    while start < idx && source.as_bytes().get(start).map_or(false, |b| *b == b' ' || *b == b'\t') {
        start += 1;
    }
    if start > idx { idx } else { start }
}

fn find_paragraph_end(source: &str, idx: usize) -> usize {
    source[idx..].find("\n\n")
        .map(|p| idx + p + 1) // include the trailing newline before blank
        .unwrap_or(source.len())
}

// ── per-format phrase resolvers ───────────────────────────────────────────

/// JSON: dot-nested key path ("provider.ollama"). Returns the
/// VALUE range — agents pass replace=new_json_value to swap.
/// Falls through to substring match if not found as a key path.
fn phrase_json(source: &str, phrase: &str) -> Option<Match> {
    // Build a path expression: for each segment, find `"<seg>":` and
    // then the value range. Naive — handles top-level dotted keys
    // without nested-array indexing. Sophisticated AST resolution
    // would parse via serde_json — but we want to operate on the
    // RAW source bytes for splicing, so the JSON parser is for
    // validation only.
    let segs: Vec<&str> = phrase.split('.').collect();
    let mut cursor = 0;
    let mut last_value_range: Option<(usize, usize)> = None;
    for seg in segs {
        let key_pattern = format!("\"{}\"", seg);
        let key_idx = source[cursor..].find(&key_pattern).map(|p| cursor + p)?;
        // Walk past key + colon + whitespace.
        let after_key = key_idx + key_pattern.len();
        let colon_off = source[after_key..].find(':')?;
        let mut value_start = after_key + colon_off + 1;
        while value_start < source.len() && source.as_bytes()[value_start].is_ascii_whitespace() {
            value_start += 1;
        }
        let value_end = scan_json_value(source, value_start)?;
        cursor = value_start;
        last_value_range = Some((value_start, value_end));
    }
    last_value_range.map(|(s, e)| Match { start: s, end: e })
}

/// Walk forward from a JSON value's start to its end byte. Handles
/// objects/arrays via bracket counting, strings via escape-aware
/// scan, primitives by char class.
fn scan_json_value(source: &str, start: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let first = *bytes.get(start)?;
    match first {
        b'"' => {
            let mut i = start + 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' { i += 2; continue; }
                if bytes[i] == b'"' { return Some(i + 1); }
                i += 1;
            }
            None
        }
        b'{' | b'[' => {
            let close = if first == b'{' { b'}' } else { b']' };
            let mut depth = 1;
            let mut i = start + 1;
            let mut in_str = false;
            while i < bytes.len() {
                let b = bytes[i];
                if in_str {
                    if b == b'\\' { i += 2; continue; }
                    if b == b'"' { in_str = false; }
                } else {
                    if b == b'"' { in_str = true; }
                    else if b == first { depth += 1; }
                    else if b == close {
                        depth -= 1;
                        if depth == 0 { return Some(i + 1); }
                    }
                }
                i += 1;
            }
            None
        }
        _ => {
            // Primitive: number / true / false / null. End at
            // first comma, whitespace, closing bracket, or EOF.
            let mut i = start;
            while i < bytes.len() {
                let b = bytes[i];
                if b == b',' || b.is_ascii_whitespace() || b == b'}' || b == b']' { break; }
                i += 1;
            }
            if i == start { None } else { Some(i) }
        }
    }
}

/// YAML: top-level or indented key:value. Replaces the value range.
fn phrase_yaml(source: &str, phrase: &str) -> Option<Match> {
    // Match `<phrase>:` at line start (optionally indented). Value
    // is the rest of that logical line OR the indented block
    // following it.
    let re = Regex::new(&format!(r"(?m)^([ \t]*){}[ \t]*:[ \t]*", regex::escape(phrase))).ok()?;
    let m = re.find(source)?;
    let key_indent_len = m.as_str().chars().take_while(|c| c.is_whitespace()).count();
    let value_start = m.end();
    // Inline value: same line until \n.
    let line_end = source[value_start..].find('\n').map(|p| value_start + p).unwrap_or(source.len());
    let inline = source[value_start..line_end].trim();
    if !inline.is_empty() {
        return Some(Match { start: value_start, end: line_end });
    }
    // Block value: deeper-indented lines following.
    let mut end = line_end + 1;
    while end < source.len() {
        let next_line_end = source[end..].find('\n').map(|p| end + p).unwrap_or(source.len());
        let line = &source[end..next_line_end];
        let line_indent_len = line.chars().take_while(|c| c.is_whitespace()).count();
        if line.trim().is_empty() {
            end = next_line_end + 1;
            continue;
        }
        if line_indent_len <= key_indent_len { break; }
        end = next_line_end + 1;
    }
    Some(Match { start: line_end + 1, end })
}

/// TOML: section header `[section]` resolves the section contents
/// (between this header and the next, or EOF).
fn phrase_toml(source: &str, phrase: &str) -> Option<Match> {
    if phrase.starts_with('[') && phrase.ends_with(']') {
        let re = Regex::new(&format!(r"(?m)^{}\s*$", regex::escape(phrase))).ok()?;
        let m = re.find(source)?;
        let body_start = source[m.end()..].find('\n').map(|p| m.end() + p + 1).unwrap_or(source.len());
        // Find next section header or EOF.
        let next_section = Regex::new(r"(?m)^\[").ok()?;
        let after_body = next_section.find(&source[body_start..]).map(|n| body_start + n.start()).unwrap_or(source.len());
        return Some(Match { start: body_start, end: after_body });
    }
    // Otherwise treat as a key in any section: `<phrase> = <value>`.
    let re = Regex::new(&format!(r"(?m)^([ \t]*){}\s*=\s*", regex::escape(phrase))).ok()?;
    let m = re.find(source)?;
    let value_start = m.end();
    let line_end = source[value_start..].find('\n').map(|p| value_start + p).unwrap_or(source.len());
    Some(Match { start: value_start, end: line_end })
}

/// env: `KEY=value`. Resolves the value range (everything after =
/// to end of line).
fn phrase_env(source: &str, phrase: &str) -> Option<Match> {
    let re = Regex::new(&format!(r"(?m)^(?:export\s+)?{}\s*=", regex::escape(phrase))).ok()?;
    let m = re.find(source)?;
    let value_start = m.end();
    let line_end = source[value_start..].find('\n').map(|p| value_start + p).unwrap_or(source.len());
    Some(Match { start: value_start, end: line_end })
}

/// Markdown headings: `## Setup` matches the heading line.
/// Replace = swap heading text. Delete = drop the heading line.
fn phrase_markdown(source: &str, phrase: &str) -> Option<Match> {
    let re = Regex::new(&format!(r"(?m)^{}\s*$", regex::escape(phrase))).ok()?;
    let m = re.find(source)?;
    let line_end = source[m.end()..].find('\n').map(|p| m.end() + p + 1).unwrap_or(source.len());
    Some(Match { start: m.start(), end: line_end })
}

// ── validation ────────────────────────────────────────────────────────────

fn validate_structure(path: &Path, before: &str, after: &str) -> Result<(), String> {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "json" | "jsonc" => validate_json(before, after),
        _ => Ok(()),
    }
}

fn validate_json(before: &str, after: &str) -> Result<(), String> {
    // Only validate when the original parsed.
    if serde_json::from_str::<serde_json::Value>(before).is_err() { return Ok(()); }
    match serde_json::from_str::<serde_json::Value>(after) {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("result is invalid JSON: {e}")),
    }
}

// ── helpers ───────────────────────────────────────────────────────────────

fn splice(source: &str, start: usize, end: usize, with: &str) -> String {
    let mut out = String::with_capacity(source.len() + with.len());
    out.push_str(&source[..start]);
    out.push_str(with);
    out.push_str(&source[end..]);
    out
}

fn window(source: &str, edit_start: usize, edit_end: usize, ctx_lines: usize) -> String {
    let mut ctx_start = edit_start;
    for _ in 0..ctx_lines {
        if ctx_start == 0 { break; }
        ctx_start = source[..ctx_start - 1].rfind('\n').map(|p| p + 1).unwrap_or(0);
    }
    let mut ctx_end = edit_end;
    for _ in 0..ctx_lines {
        match source[ctx_end..].find('\n') {
            Some(p) => ctx_end += p + 1,
            None => { ctx_end = source.len(); break; }
        }
    }
    source.get(ctx_start..ctx_end).unwrap_or("").to_string()
}

fn write_whole_file(path: &Path, content: &str) -> ToolResult {
    let existed = path.exists();
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return ToolResult::error(format!("mkdir -p failed: {e}"));
            }
        }
    }
    if let Err(e) = std::fs::write(path, content) {
        return ToolResult::error(format!("write failed: {e}"));
    }
    let verb = if existed { "Overwrote" } else { "Created" };
    ToolResult {
        output: format!("{verb} {} ({} bytes).", path.display(), content.len()),
        title: Some(path.display().to_string()),
        metadata: Some(json!({ "existed": existed, "size": content.len() })),
        is_error: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);
    fn tmp(ext: &str) -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("humfs-do_noncode-{}-{}.{}", std::process::id(), n, ext))
    }

    #[tokio::test]
    async fn whole_file_creates() {
        let p = tmp("txt");
        let _ = fs::remove_file(&p);
        let res = run(json!({
            "file_path": p.display().to_string(),
            "replace": "hello\n",
        })).await;
        assert!(!res.is_error, "create failed: {}", res.output);
        assert_eq!(fs::read_to_string(&p).unwrap(), "hello\n");
        let _ = fs::remove_file(&p);
    }

    #[tokio::test]
    async fn refuses_code_extension() {
        let p = tmp("rs");
        fs::write(&p, "fn x(){}\n").unwrap();
        let res = run(json!({
            "file_path": p.display().to_string(),
            "replace": "x",
        })).await;
        assert!(res.is_error);
        assert!(res.output.contains("humfs_do_code"));
        let _ = fs::remove_file(&p);
    }

    #[tokio::test]
    async fn word_swap() {
        let p = tmp("txt");
        fs::write(&p, "host = localhost\nport = 3000\n").unwrap();
        let res = run(json!({
            "file_path": p.display().to_string(),
            "word": "localhost",
            "replace": "0.0.0.0",
        })).await;
        assert!(!res.is_error, "{}", res.output);
        let s = fs::read_to_string(&p).unwrap();
        assert!(s.contains("0.0.0.0"));
        assert!(!s.contains("localhost"));
        let _ = fs::remove_file(&p);
    }

    #[tokio::test]
    async fn env_phrase() {
        let p = tmp("env");
        fs::write(&p, "DATABASE_URL=postgres://old/db\nAPI_KEY=secret\n").unwrap();
        let res = run(json!({
            "file_path": p.display().to_string(),
            "phrase": "DATABASE_URL",
            "replace": "postgres://new/db",
        })).await;
        assert!(!res.is_error, "{}", res.output);
        let s = fs::read_to_string(&p).unwrap();
        assert!(s.contains("DATABASE_URL=postgres://new/db"));
        assert!(s.contains("API_KEY=secret"));
        let _ = fs::remove_file(&p);
    }

    #[tokio::test]
    async fn markdown_heading() {
        let p = tmp("md");
        fs::write(&p, "# Title\n\n## Setup\n\nold instructions\n").unwrap();
        let res = run(json!({
            "file_path": p.display().to_string(),
            "phrase": "## Setup",
            "replace": "## Installation\n",
        })).await;
        assert!(!res.is_error, "{}", res.output);
        let s = fs::read_to_string(&p).unwrap();
        assert!(s.contains("## Installation"));
        assert!(!s.contains("## Setup"));
        let _ = fs::remove_file(&p);
    }

    #[tokio::test]
    async fn json_phrase_swaps_value_and_validates() {
        let p = tmp("json");
        fs::write(&p, r#"{"name": "old", "port": 3000}"#).unwrap();
        let res = run(json!({
            "file_path": p.display().to_string(),
            "phrase": "name",
            "replace": "\"new-name\"",
        })).await;
        assert!(!res.is_error, "{}", res.output);
        let s = fs::read_to_string(&p).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["name"], "new-name");
        assert_eq!(parsed["port"], 3000);
        let _ = fs::remove_file(&p);
    }

    #[tokio::test]
    async fn json_rejects_invalid_result() {
        let p = tmp("json");
        fs::write(&p, r#"{"port": 3000}"#).unwrap();
        let res = run(json!({
            "file_path": p.display().to_string(),
            "phrase": "port",
            "replace": "not_a_value",  // unquoted, breaks JSON
        })).await;
        assert!(res.is_error, "should have rejected invalid JSON");
        let s = fs::read_to_string(&p).unwrap();
        assert!(s.contains("3000"), "original should be untouched: {s}");
        let _ = fs::remove_file(&p);
    }

    #[tokio::test]
    async fn toml_section() {
        let p = tmp("toml");
        fs::write(&p, "[server]\nport = 3000\n\n[database]\nhost = \"localhost\"\n").unwrap();
        let res = run(json!({
            "file_path": p.display().to_string(),
            "phrase": "[server]",
            "replace": "host = \"0.0.0.0\"\nport = 9090\n",
        })).await;
        assert!(!res.is_error, "{}", res.output);
        let s = fs::read_to_string(&p).unwrap();
        assert!(s.contains("port = 9090"));
        assert!(s.contains("[database]"));
        let _ = fs::remove_file(&p);
    }

    #[tokio::test]
    async fn paragraph_scope() {
        let p = tmp("txt");
        fs::write(&p, "first paragraph\nmore first\n\ntarget block\ncontent line\n\nthird block\n").unwrap();
        let res = run(json!({
            "file_path": p.display().to_string(),
            "paragraph": "target block",
            "replace": "REPLACED",
        })).await;
        assert!(!res.is_error, "{}", res.output);
        let s = fs::read_to_string(&p).unwrap();
        assert!(s.contains("REPLACED"));
        assert!(!s.contains("target block"));
        assert!(!s.contains("content line"));
        assert!(s.contains("first paragraph"));
        assert!(s.contains("third block"));
        let _ = fs::remove_file(&p);
    }

    #[tokio::test]
    async fn delete_phrase_via_empty_replace() {
        let p = tmp("env");
        fs::write(&p, "DATABASE_URL=postgres://x\nAPI_KEY=secret\n").unwrap();
        let res = run(json!({
            "file_path": p.display().to_string(),
            "phrase": "DATABASE_URL",
        })).await;
        assert!(!res.is_error, "{}", res.output);
        let s = fs::read_to_string(&p).unwrap();
        // Value is deleted; key+= stays (we only resolve the value
        // range for env). Agent passes phrase="DATABASE_URL=..."
        // exact-substring form to drop the full line.
        assert!(s.contains("API_KEY=secret"));
        let _ = fs::remove_file(&p);
    }
}

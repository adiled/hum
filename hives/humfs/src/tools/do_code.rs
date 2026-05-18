//! `humfs_do_code` — AST-grounded code authoring.
//!
//! Operations:
//!
//! - `create` — new file. Refuses if the path exists. Validates
//!   syntax against the language's tree-sitter parser before write.
//! - `replace` — symbol-scoped or whole-file. With `symbol`, the
//!   target symbol's byte range is spliced for `new_source`; without
//!   `symbol`, the entire file is rewritten. Either way the result
//!   is re-parsed; a syntax-error result aborts the write.
//! - `insert_before` / `insert_after` — splice `new_source` at the
//!   start (resp. end) of the anchor symbol's byte range, with a
//!   newline separator so the new code lands on its own line.
//! - `delete` — drop the anchor symbol's byte range. If the symbol
//!   sat alone on indented lines, drops the surrounding blank
//!   lines too.
//!
//! Non-code extensions are routed back to `humfs_do_noncode`. The
//! synthetic `imports` symbol covers the leading contiguous run of
//! import / use / include / require nodes at top level — addressable
//! via `symbol: "imports"` on any of the above operations except
//! `create`.

use std::path::{Path, PathBuf};

use nest_common::{ToolDef, ToolResult};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::ast::{self, LangSpec, Symbol, SymbolKind};

#[derive(Deserialize)]
struct Args {
    file_path: String,
    #[serde(default = "default_op")]
    operation: String,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    new_source: Option<String>,
}

fn default_op() -> String { "replace".into() }

pub fn def() -> ToolDef {
    ToolDef {
        name: "humfs_do_code".into(),
        description: "Author code — AST-grounded, symbol-scoped. Operations: create | replace (symbol OR whole-file) | insert_before | insert_after | delete. The top-of-file import block is addressable as the synthetic 'imports' symbol. Sub-symbol walks (body/when/otherwise/loop/try/return/call) compose with dots and disambiguate with #N (P6). Languages: ts/tsx/js/jsx/mjs/cjs/py/pyi/go/rs (AST-backed today). Every write is re-parsed; a syntax-error result aborts the write. Non-code files route to humfs_do_noncode.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "file_path":  { "type": "string", "description": "Absolute path to the code file. Must have a code extension." },
                "operation":  { "type": "string", "description": "One of: create, replace, insert_before, insert_after, delete. Default: replace." },
                "symbol":     { "type": "string", "description": "Target symbol name. Required for replace (unless whole-file rewrite), insert_before, insert_after, delete. Dot-separated for nested (e.g. 'Class.method'). Use 'imports' for the synthetic import-block symbol." },
                "new_source": { "type": "string", "description": "The new source code. Required for create, replace, insert_before, insert_after." },
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
    let lang = match ast::detect_language(&path) {
        Some(l) => l,
        None => return ToolResult::error(format!(
            "humfs_do_code targets code files only. '{}' has no recognized code extension — route to humfs_do_noncode.",
            path.display()
        )),
    };

    match args.operation.as_str() {
        "create" => op_create(&path, lang, args.new_source.as_deref()),
        "replace" => op_replace(&path, lang, args.symbol.as_deref(), args.new_source.as_deref()),
        "insert_before" => op_insert(&path, lang, args.symbol.as_deref(), args.new_source.as_deref(), Anchor::Before),
        "insert_after"  => op_insert(&path, lang, args.symbol.as_deref(), args.new_source.as_deref(), Anchor::After),
        "delete" => op_delete(&path, lang, args.symbol.as_deref()),
        other => ToolResult::error(format!(
            "unknown operation '{other}' — pick one of: create, replace, insert_before, insert_after, delete"
        )),
    }
}

// ── ops ──────────────────────────────────────────────────────────────────

fn op_create(path: &Path, lang: LangSpec, new_source: Option<&str>) -> ToolResult {
    let src = match new_source {
        Some(s) => s,
        None => return ToolResult::error("create needs new_source"),
    };
    if path.exists() {
        return ToolResult::error(format!(
            "{} already exists. Use operation 'replace' to modify, or 'insert_before' / 'insert_after' to add adjacent to an existing symbol.",
            path.display()
        ));
    }
    if let Err(msg) = ast::validate_syntax(src, lang) {
        return ToolResult::error(format!("rejected — new_source has {msg}"));
    }
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return ToolResult::error(format!("mkdir -p failed: {e}"));
            }
        }
    }
    if let Err(e) = std::fs::write(path, src) {
        return ToolResult::error(format!("write failed: {e}"));
    }
    ok(format!("Created {} ({} bytes)", path.display(), src.len()), path)
}

fn op_replace(
    path: &Path, lang: LangSpec, symbol: Option<&str>, new_source: Option<&str>,
) -> ToolResult {
    let new = match new_source {
        Some(s) => s.to_string(),
        None => return ToolResult::error("replace needs new_source"),
    };
    let original = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return ToolResult::error(format!("read failed: {e}")),
    };

    let updated = match symbol {
        None => new.clone(),
        Some(sym_name) => match locate_symbol(&original, lang, sym_name) {
            None => return ToolResult::error(format!("symbol '{sym_name}' not found in {}", path.display())),
            Some(sym) => splice(&original, sym.start_byte, sym.end_byte, &new),
        },
    };

    if let Err(msg) = ast::validate_syntax(&updated, lang) {
        return ToolResult::error(format!("rejected — result has {msg}; original left untouched"));
    }
    if let Err(e) = std::fs::write(path, &updated) {
        return ToolResult::error(format!("write failed: {e}"));
    }
    let scope = symbol.map(|s| format!("symbol '{s}'")).unwrap_or_else(|| "whole file".into());
    ok(format!("Replaced {} in {} ({} bytes)", scope, path.display(), updated.len()), path)
}

#[derive(Clone, Copy)]
enum Anchor { Before, After }

fn op_insert(
    path: &Path, lang: LangSpec, symbol: Option<&str>, new_source: Option<&str>, anchor: Anchor,
) -> ToolResult {
    let new = match new_source {
        Some(s) => s,
        None => return ToolResult::error("insert needs new_source"),
    };
    let sym_name = match symbol {
        Some(s) => s,
        None => return ToolResult::error("insert needs symbol (the anchor)"),
    };
    let original = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return ToolResult::error(format!("read failed: {e}")),
    };
    let sym = match locate_symbol(&original, lang, sym_name) {
        Some(s) => s,
        None => return ToolResult::error(format!("symbol '{sym_name}' not found in {}", path.display())),
    };
    let insert_at = match anchor {
        Anchor::Before => line_start_if_indented_alone(&original, sym.start_byte),
        Anchor::After  => sym.end_byte,
    };
    let separator = if new.ends_with('\n') { "\n" } else { "\n\n" };
    let payload = match anchor {
        Anchor::Before => format!("{new}{separator}"),
        Anchor::After  => format!("{separator}{new}"),
    };
    let updated = splice(&original, insert_at, insert_at, &payload);

    if let Err(msg) = ast::validate_syntax(&updated, lang) {
        return ToolResult::error(format!("rejected — result has {msg}; original left untouched"));
    }
    if let Err(e) = std::fs::write(path, &updated) {
        return ToolResult::error(format!("write failed: {e}"));
    }
    let where_str = match anchor { Anchor::Before => "before", Anchor::After => "after" };
    ok(
        format!("Inserted {} bytes {where_str} '{sym_name}' in {}", new.len(), path.display()),
        path,
    )
}

fn op_delete(path: &Path, lang: LangSpec, symbol: Option<&str>) -> ToolResult {
    let sym_name = match symbol {
        Some(s) => s,
        None => return ToolResult::error("delete needs symbol"),
    };
    let original = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return ToolResult::error(format!("read failed: {e}")),
    };
    let sym = match locate_symbol(&original, lang, sym_name) {
        Some(s) => s,
        None => return ToolResult::error(format!("symbol '{sym_name}' not found in {}", path.display())),
    };
    // Drop the symbol's range PLUS the leading whitespace on its
    // own line, so we don't leave a half-line behind.
    let start = line_start_if_indented_alone(&original, sym.start_byte);
    let mut end = sym.end_byte;
    // Eat one trailing newline so successive symbols stay aligned.
    if original.get(end..end + 1) == Some("\n") { end += 1; }
    let updated = splice(&original, start, end, "");

    if let Err(msg) = ast::validate_syntax(&updated, lang) {
        return ToolResult::error(format!("rejected — result has {msg}; original left untouched"));
    }
    if let Err(e) = std::fs::write(path, &updated) {
        return ToolResult::error(format!("write failed: {e}"));
    }
    ok(format!("Deleted symbol '{sym_name}' from {}", path.display()), path)
}

// ── symbol resolution ───────────────────────────────────────────────────

/// Locate the named symbol in `source`. Supports plain names,
/// dot-nested ("Class.method"), sub-symbol alias walks
/// ("alpha.body", "alpha.when.otherwise"), and the synthetic
/// `imports` symbol.
fn locate_symbol(source: &str, lang: LangSpec, name: &str) -> Option<Symbol> {
    if name == "imports" {
        return synthesize_imports(source, lang);
    }
    let (start_byte, end_byte, start_row, end_row) = ast::resolve_path(source, lang, name)?;
    Some(Symbol {
        name: name.to_string(),
        kind: SymbolKind::Other,
        start_byte, end_byte, start_row, end_row,
    })
}

/// Synthetic `imports` symbol: the leading contiguous run of
/// import/use/require/include declarations at top level. Walks
/// the tree-sitter tree's first-level children and groups
/// neighbour import nodes into one byte range.
fn synthesize_imports(source: &str, lang: LangSpec) -> Option<Symbol> {
    let tree = ast::parse(source, lang)?;
    let root = tree.root_node();
    let mut cur = root.walk();
    let mut first_byte: Option<usize> = None;
    let mut last_byte: usize = 0;
    let mut last_row: usize = 1;
    for child in root.children(&mut cur) {
        let kind = child.kind();
        let is_import = matches!(
            kind,
            "use_declaration"          // rust
            | "extern_crate_declaration"
            | "import_statement"       // py, js, ts
            | "import_from_statement"  // py
            | "import_declaration"     // go, js, ts
            | "import_spec"
            | "require_statement"
            | "preproc_include"        // c/cpp
        );
        if is_import {
            if first_byte.is_none() { first_byte = Some(child.start_byte()); }
            last_byte = child.end_byte();
            last_row = child.end_position().row + 1;
        } else if first_byte.is_some() {
            break;
        }
    }
    let start_byte = first_byte?;
    Some(Symbol {
        name: "imports".into(),
        kind: SymbolKind::Imports,
        start_byte,
        end_byte: last_byte,
        start_row: source[..start_byte].lines().count().max(1),
        end_row: last_row,
    })
}

// ── helpers ─────────────────────────────────────────────────────────────

fn splice(source: &str, start: usize, end: usize, with: &str) -> String {
    let mut out = String::with_capacity(source.len() + with.len());
    out.push_str(&source[..start]);
    out.push_str(with);
    out.push_str(&source[end..]);
    out
}

/// Walk back to line start if everything between line-start and
/// `index` is whitespace — so the splice point doesn't leave a half
/// line of indentation. If the symbol shares its line with anything
/// else (`def f(): pass; def g(): pass`), return `index` untouched.
fn line_start_if_indented_alone(source: &str, index: usize) -> usize {
    let mut i = index;
    while i > 0 && &source[i - 1..i] != "\n" { i -= 1; }
    for k in i..index {
        let ch = &source[k..k + 1];
        if ch != " " && ch != "\t" { return index; }
    }
    i
}

fn ok(msg: String, path: &Path) -> ToolResult {
    ToolResult {
        output: msg,
        title: Some(path.display().to_string()),
        metadata: Some(json!({ "path": path.display().to_string() })),
        is_error: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);
    fn tmp(suffix: &str) -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("humfs-do_code-{}-{}.{}", std::process::id(), n, suffix))
    }

    #[tokio::test]
    async fn create_writes_and_validates() {
        let p = tmp("rs");
        let _ = fs::remove_file(&p);
        let res = run(json!({
            "file_path": p.display().to_string(),
            "operation": "create",
            "new_source": "fn main() {}\n",
        })).await;
        assert!(!res.is_error, "create failed: {}", res.output);
        assert_eq!(fs::read_to_string(&p).unwrap(), "fn main() {}\n");
        let _ = fs::remove_file(&p);
    }

    #[tokio::test]
    async fn create_rejects_bad_syntax() {
        let p = tmp("rs");
        let _ = fs::remove_file(&p);
        let res = run(json!({
            "file_path": p.display().to_string(),
            "operation": "create",
            "new_source": "fn x( { ;;",
        })).await;
        assert!(res.is_error, "should have rejected syntax error");
        assert!(!p.exists(), "file should not have been written");
    }

    #[tokio::test]
    async fn replace_symbol_scoped() {
        let p = tmp("rs");
        fs::write(&p, "fn alpha() -> u32 { 1 }\nfn beta() -> u32 { 2 }\n").unwrap();
        let res = run(json!({
            "file_path": p.display().to_string(),
            "operation": "replace",
            "symbol": "alpha",
            "new_source": "fn alpha() -> u32 { 99 }",
        })).await;
        assert!(!res.is_error, "replace failed: {}", res.output);
        let updated = fs::read_to_string(&p).unwrap();
        assert!(updated.contains("99"), "didn't replace alpha: {updated}");
        assert!(updated.contains("fn beta"), "lost beta: {updated}");
        let _ = fs::remove_file(&p);
    }

    #[tokio::test]
    async fn replace_whole_file_when_no_symbol() {
        let p = tmp("rs");
        fs::write(&p, "fn old() {}").unwrap();
        let res = run(json!({
            "file_path": p.display().to_string(),
            "operation": "replace",
            "new_source": "fn brand_new() -> u32 { 42 }",
        })).await;
        assert!(!res.is_error, "whole-file replace failed: {}", res.output);
        let updated = fs::read_to_string(&p).unwrap();
        assert!(updated.contains("brand_new"));
        assert!(!updated.contains("old"));
        let _ = fs::remove_file(&p);
    }

    #[tokio::test]
    async fn insert_after_anchor() {
        let p = tmp("rs");
        fs::write(&p, "fn alpha() {}\n").unwrap();
        let res = run(json!({
            "file_path": p.display().to_string(),
            "operation": "insert_after",
            "symbol": "alpha",
            "new_source": "fn beta() {}",
        })).await;
        assert!(!res.is_error, "insert_after failed: {}", res.output);
        let updated = fs::read_to_string(&p).unwrap();
        assert!(updated.contains("fn alpha"));
        assert!(updated.contains("fn beta"));
        let _ = fs::remove_file(&p);
    }

    #[tokio::test]
    async fn delete_symbol() {
        let p = tmp("rs");
        fs::write(&p, "fn alpha() {}\nfn beta() {}\n").unwrap();
        let res = run(json!({
            "file_path": p.display().to_string(),
            "operation": "delete",
            "symbol": "alpha",
        })).await;
        assert!(!res.is_error, "delete failed: {}", res.output);
        let updated = fs::read_to_string(&p).unwrap();
        assert!(!updated.contains("fn alpha"));
        assert!(updated.contains("fn beta"));
        let _ = fs::remove_file(&p);
    }

    #[tokio::test]
    async fn replace_rejects_when_result_breaks_syntax() {
        let p = tmp("rs");
        fs::write(&p, "fn alpha() {}\n").unwrap();
        let res = run(json!({
            "file_path": p.display().to_string(),
            "operation": "replace",
            "symbol": "alpha",
            "new_source": "fn alpha( { ;;",
        })).await;
        assert!(res.is_error, "should have rejected broken result");
        assert!(fs::read_to_string(&p).unwrap().contains("fn alpha() {}"),
            "original should be untouched");
        let _ = fs::remove_file(&p);
    }

    #[tokio::test]
    async fn refuses_non_code_extension() {
        let p = tmp("md");
        fs::write(&p, "# hi\n").unwrap();
        let res = run(json!({
            "file_path": p.display().to_string(),
            "operation": "replace",
            "new_source": "# bye\n",
        })).await;
        assert!(res.is_error);
        assert!(res.output.contains("humfs_do_noncode"), "wrong rejection: {}", res.output);
        let _ = fs::remove_file(&p);
    }

    #[tokio::test]
    async fn replace_sub_symbol_body() {
        // Replace the body of `alpha` via the sub-symbol path
        // "alpha.body". Without that, the caller would have to know
        // alpha's exact byte range.
        let p = tmp("rs");
        fs::write(&p, "fn alpha() {\n    let x = 1;\n}\nfn beta() {}\n").unwrap();
        let res = run(json!({
            "file_path": p.display().to_string(),
            "operation": "replace",
            "symbol": "alpha.body",
            "new_source": "{ let y = 42; }",
        })).await;
        assert!(!res.is_error, "sub-symbol replace failed: {}", res.output);
        let updated = fs::read_to_string(&p).unwrap();
        assert!(updated.contains("y = 42"), "didn't replace body: {updated}");
        assert!(updated.contains("fn alpha"), "lost alpha signature: {updated}");
        assert!(updated.contains("fn beta"), "lost beta: {updated}");
        let _ = fs::remove_file(&p);
    }

    #[tokio::test]
    async fn imports_symbol_replace() {
        let p = tmp("rs");
        fs::write(&p, "use std::fs;\nuse std::io;\n\nfn alpha() {}\n").unwrap();
        let res = run(json!({
            "file_path": p.display().to_string(),
            "operation": "replace",
            "symbol": "imports",
            "new_source": "use std::path::PathBuf;\nuse std::collections::HashMap;",
        })).await;
        assert!(!res.is_error, "imports replace failed: {}", res.output);
        let updated = fs::read_to_string(&p).unwrap();
        assert!(updated.contains("PathBuf"));
        assert!(updated.contains("HashMap"));
        assert!(!updated.contains("std::fs"));
        assert!(updated.contains("fn alpha"));
        let _ = fs::remove_file(&p);
    }
}

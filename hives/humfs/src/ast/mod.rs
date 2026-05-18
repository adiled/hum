//! AST infrastructure — tree-sitter parsers + symbol extraction.
//!
//! One parser registry keyed by file extension. Each language ships:
//!
//! - a tree-sitter `Language` (loaded from the parser crate),
//! - a tree-sitter `Query` over named-symbol patterns (functions,
//!   classes/structs, methods, top-level bindings, types, enums),
//! - capture-name → `SymbolKind` mapping.
//!
//! Symbol extraction parses once, runs the query, walks captures
//! into a flat `Vec<Symbol>` sorted by byte range. P4 consumes this
//! for the `read` outline + `symbol` modifier; P5 consumes it for
//! `do_code` symbol-scoped writes.

use std::path::Path;

use tree_sitter::{Language, Node, Parser, Query, QueryCursor, StreamingIterator};

pub mod outline;
pub mod query;
pub mod symbol;

pub use symbol::{Symbol, SymbolKind};

/// Recognize a code language from a file path. Returns None for
/// unsupported extensions; caller falls back to the text path.
pub fn detect_language(path: &Path) -> Option<LangSpec> {
    let ext = path.extension().and_then(|s| s.to_str())?.to_ascii_lowercase();
    match ext.as_str() {
        "rs" => Some(LangSpec::Rust),
        "py" | "pyi" => Some(LangSpec::Python),
        "go" => Some(LangSpec::Go),
        "js" | "jsx" | "mjs" | "cjs" => Some(LangSpec::JavaScript),
        "ts" => Some(LangSpec::TypeScript),
        "tsx" => Some(LangSpec::Tsx),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LangSpec {
    Rust,
    Python,
    Go,
    JavaScript,
    TypeScript,
    Tsx,
}

impl LangSpec {
    pub fn tree_sitter_language(self) -> Language {
        match self {
            LangSpec::Rust       => tree_sitter_rust::LANGUAGE.into(),
            LangSpec::Python     => tree_sitter_python::LANGUAGE.into(),
            LangSpec::Go         => tree_sitter_go::LANGUAGE.into(),
            LangSpec::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            LangSpec::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            LangSpec::Tsx        => tree_sitter_typescript::LANGUAGE_TSX.into(),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            LangSpec::Rust       => "rust",
            LangSpec::Python     => "python",
            LangSpec::Go         => "go",
            LangSpec::JavaScript => "javascript",
            LangSpec::TypeScript => "typescript",
            LangSpec::Tsx        => "tsx",
        }
    }
}

/// Parse a source string against the given language. Returns a
/// parser-owned tree the caller can query.
pub fn parse(source: &str, lang: LangSpec) -> Option<tree_sitter::Tree> {
    let mut parser = Parser::new();
    parser.set_language(&lang.tree_sitter_language()).ok()?;
    parser.parse(source, None)
}

/// Run the given language's symbol query over `source`, returning
/// every captured symbol sorted by start byte. Children stay in the
/// returned tree's order; outline formatting handles indentation.
pub fn file_symbols(source: &str, lang: LangSpec) -> Vec<Symbol> {
    let tree = match parse(source, lang) {
        Some(t) => t,
        None => return vec![],
    };
    let language = lang.tree_sitter_language();
    let query_src = query::symbol_query(lang);
    let q = match Query::new(&language, query_src) {
        Ok(q) => q,
        Err(_) => return vec![],
    };
    let capture_names: Vec<&str> = (0..q.capture_names().len())
        .map(|i| q.capture_names()[i])
        .collect();

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&q, tree.root_node(), source.as_bytes());
    let mut out: Vec<Symbol> = Vec::new();
    while let Some(m) = matches.next() {
        let mut name: Option<String> = None;
        let mut node_for_range: Option<Node> = None;
        let mut kind = SymbolKind::Other;
        for cap in m.captures {
            let cap_name = capture_names[cap.index as usize];
            // Convention: `@def` captures the enclosing definition node
            // (whose byte range we want), `@name` captures the
            // identifier. Other capture suffixes (.fn / .class / etc.)
            // act as the kind tag.
            if let Some(tag) = cap_name.strip_suffix(".def") {
                node_for_range = Some(cap.node);
                kind = SymbolKind::from_tag(tag);
            } else if cap_name.ends_with(".name") {
                if let Ok(s) = cap.node.utf8_text(source.as_bytes()) {
                    name = Some(s.to_string());
                }
            }
        }
        if let (Some(name), Some(node)) = (name, node_for_range) {
            let start_byte = node.start_byte();
            let end_byte = node.end_byte();
            let start_row = node.start_position().row + 1;
            let end_row = node.end_position().row + 1;
            out.push(Symbol { name, kind, start_byte, end_byte, start_row, end_row });
        }
    }
    out.sort_by_key(|s| s.start_byte);
    out
}

/// Find the smallest symbol enclosing the given byte offset.
/// Useful for annotating regex hits in `humfs_read` with the
/// function / class they sit inside.
pub fn enclosing_symbol(symbols: &[Symbol], byte: usize) -> Option<&Symbol> {
    let mut best: Option<&Symbol> = None;
    for s in symbols {
        if s.start_byte <= byte && byte < s.end_byte {
            best = match best {
                Some(b) if (b.end_byte - b.start_byte) <= (s.end_byte - s.start_byte) => Some(b),
                _ => Some(s),
            };
        }
    }
    best
}

/// Return Err with a one-line message if the source has any syntax
/// errors per the given language's parser. Used as a post-write
/// validation gate by `humfs_do_code` (P5).
pub fn validate_syntax(source: &str, lang: LangSpec) -> Result<(), String> {
    let tree = parse(source, lang).ok_or_else(|| "parser unavailable".to_string())?;
    let root = tree.root_node();
    if root.has_error() {
        // Walk to the first ERROR / missing node for a helpful location.
        let (row, col) = first_error_position(root);
        return Err(format!("syntax error at line {}, column {}", row + 1, col + 1));
    }
    Ok(())
}

fn first_error_position(node: Node) -> (usize, usize) {
    if node.is_error() || node.is_missing() {
        let p = node.start_position();
        return (p.row, p.column);
    }
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        if child.has_error() {
            return first_error_position(child);
        }
    }
    let p = node.start_position();
    (p.row, p.column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_basic_extensions() {
        assert_eq!(detect_language(Path::new("/x.rs")), Some(LangSpec::Rust));
        assert_eq!(detect_language(Path::new("/x.py")), Some(LangSpec::Python));
        assert_eq!(detect_language(Path::new("/x.tsx")), Some(LangSpec::Tsx));
        assert_eq!(detect_language(Path::new("/x.md")), None);
    }

    #[test]
    fn rust_parses_and_finds_functions() {
        let src = r#"
            fn alpha() -> i32 { 1 }
            fn beta(x: i32) -> i32 { x + 1 }
            struct Carrier { field: u32 }
            impl Carrier { fn method(&self) -> u32 { self.field } }
        "#;
        let syms = file_symbols(src, LangSpec::Rust);
        let names: Vec<_> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"alpha"), "alpha missing: {:?}", names);
        assert!(names.contains(&"beta"), "beta missing: {:?}", names);
        assert!(names.contains(&"Carrier"), "Carrier missing: {:?}", names);
        assert!(names.contains(&"method"), "method missing: {:?}", names);
    }

    #[test]
    fn rust_syntax_error_detected() {
        let bad = "fn x( { not valid }";
        assert!(validate_syntax(bad, LangSpec::Rust).is_err());
    }

    #[test]
    fn rust_clean_passes_syntax() {
        let ok = "fn x() -> u32 { 42 }";
        assert!(validate_syntax(ok, LangSpec::Rust).is_ok());
    }

    #[test]
    fn python_finds_functions_and_classes() {
        let src = r#"
def alpha():
    return 1

class Beta:
    def method(self):
        return 2
"#;
        let syms = file_symbols(src, LangSpec::Python);
        let names: Vec<_> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"alpha"), "alpha missing: {:?}", names);
        assert!(names.contains(&"Beta"),  "Beta missing: {:?}", names);
        assert!(names.contains(&"method"), "method missing: {:?}", names);
    }

    #[test]
    fn typescript_finds_functions_classes_consts() {
        let src = r#"
            function alpha(): number { return 1; }
            class Beta { method(): number { return 2; } }
            const GAMMA = 42;
            type Delta = string;
        "#;
        let syms = file_symbols(src, LangSpec::TypeScript);
        let names: Vec<_> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"alpha"), "alpha: {:?}", names);
        assert!(names.contains(&"Beta"),  "Beta: {:?}", names);
        assert!(names.contains(&"method"), "method: {:?}", names);
        assert!(names.contains(&"GAMMA"), "GAMMA: {:?}", names);
        assert!(names.contains(&"Delta"), "Delta: {:?}", names);
    }
}

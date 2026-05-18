//! Sub-symbol walking. 7-word vocabulary that walks into the AST
//! beyond a named symbol's range:
//!
//! - **`body`** — inside-block of any compound (function body, then-
//!   branch of if, loop body, try block). Resolves the `body` /
//!   `consequence` named field first; falls back to the first
//!   block-typed direct child for the language.
//! - **`when`** — an if (`if_statement` / `if_expression`).
//! - **`otherwise`** — the alternate branch: `else` of an if (its
//!   `alternative` field), `catch` of a try (catch-clause typed
//!   descendant).
//! - **`loop`** — for / while / loop / do.
//! - **`try`** — try construct.
//! - **`return`** — return statement.
//! - **`call`** — function call.
//!
//! Compose with dots; disambiguate with `#N`. Walk is document-order;
//! `#N` counts distinct siblings, not nested matches inside earlier
//! matches (a call inside a call is `call#1.call`, not `call#2`).

use std::collections::HashSet;

use tree_sitter::Node;

use crate::ast::LangSpec;

/// One segment after the named symbol. `alias` is one of the 7
/// vocabulary words; `occurrence` is the 1-based ordinal (1 for
/// no-`#N` segments).
#[derive(Debug, Clone)]
pub struct AliasSegment {
    pub alias: String,
    pub occurrence: usize,
}

/// Parse "when#2" → `AliasSegment { alias: "when", occurrence: 2 }`.
pub fn parse_segment(raw: &str) -> Option<AliasSegment> {
    let (alias, occurrence) = match raw.split_once('#') {
        Some((a, n)) => (a.to_string(), n.parse().ok()?),
        None => (raw.to_string(), 1usize),
    };
    if occurrence == 0 { return None; }
    if !matches!(alias.as_str(), "body" | "when" | "otherwise" | "loop" | "try" | "return" | "call") {
        return None;
    }
    Some(AliasSegment { alias, occurrence })
}

/// Resolve a path of alias segments under `root`, returning the
/// final matching node. Each segment's match becomes the scope for
/// the next segment.
pub fn resolve_subpath<'tree>(
    mut root: Node<'tree>,
    segs: &[AliasSegment],
    lang: LangSpec,
) -> Option<Node<'tree>> {
    for seg in segs {
        root = resolve_segment(root, seg, lang)?;
    }
    Some(root)
}

fn resolve_segment<'tree>(node: Node<'tree>, seg: &AliasSegment, lang: LangSpec) -> Option<Node<'tree>> {
    match seg.alias.as_str() {
        "body" => if seg.occurrence == 1 { resolve_body(node, lang) } else { None },
        "otherwise" => if seg.occurrence == 1 { resolve_otherwise(node) } else { None },
        other => {
            let types = alias_types(other, lang)?;
            find_nth_descendant(node, &types, seg.occurrence)
        }
    }
}

// ── body / otherwise — context-dependent on the parent node ────────────

fn resolve_body<'tree>(node: Node<'tree>, lang: LangSpec) -> Option<Node<'tree>> {
    for field in &["body", "consequence"] {
        if let Some(n) = node.child_by_field_name(field) {
            return Some(n);
        }
    }
    let types = block_types(lang);
    let mut cur = node.walk();
    for c in node.children(&mut cur) {
        if types.contains(c.kind()) {
            return Some(c);
        }
    }
    None
}

fn resolve_otherwise<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    if let Some(n) = node.child_by_field_name("alternative") {
        return Some(n);
    }
    let catch_types: HashSet<&'static str> = [
        "catch_clause", "except_clause", "else_clause", "rescue",
        "catch_block", "catch", "handler",
    ].into_iter().collect();
    let mut cur = node.walk();
    for c in node.children(&mut cur) {
        if catch_types.contains(c.kind()) {
            return Some(c);
        }
    }
    None
}

// ── document-order Nth descendant by node-kind set ─────────────────────

fn find_nth_descendant<'tree>(
    root: Node<'tree>, types: &HashSet<&'static str>, occurrence: usize,
) -> Option<Node<'tree>> {
    let mut count = 0;
    walk(root, types, occurrence, &mut count)
}

fn walk<'tree>(
    node: Node<'tree>, types: &HashSet<&'static str>, occurrence: usize, count: &mut usize,
) -> Option<Node<'tree>> {
    let mut cur = node.walk();
    for c in node.children(&mut cur) {
        if types.contains(c.kind()) {
            *count += 1;
            if *count == occurrence { return Some(c); }
            // Per spec: do not descend into matched nodes — a call
            // inside a call is `call#1.call`, not `call#2` of the
            // enclosing scope.
            continue;
        }
        if let Some(found) = walk(c, types, occurrence, count) {
            return Some(found);
        }
    }
    None
}

// ── per-language tables ────────────────────────────────────────────────

fn block_types(lang: LangSpec) -> HashSet<&'static str> {
    match lang {
        LangSpec::Rust   => ["block"].into_iter().collect(),
        LangSpec::Python => ["block"].into_iter().collect(),
        LangSpec::Go     => ["block"].into_iter().collect(),
        LangSpec::JavaScript | LangSpec::TypeScript | LangSpec::Tsx =>
            ["statement_block", "class_body"].into_iter().collect(),
    }
}

fn alias_types(alias: &str, lang: LangSpec) -> Option<HashSet<&'static str>> {
    let set: &[&'static str] = match (alias, lang) {
        ("when", LangSpec::Rust)   => &["if_expression"],
        ("when", LangSpec::Python) => &["if_statement"],
        ("when", LangSpec::Go)     => &["if_statement"],
        ("when", LangSpec::JavaScript) | ("when", LangSpec::TypeScript) | ("when", LangSpec::Tsx)
            => &["if_statement"],

        ("loop", LangSpec::Rust)   => &["for_expression", "while_expression", "loop_expression"],
        ("loop", LangSpec::Python) => &["for_statement", "while_statement"],
        ("loop", LangSpec::Go)     => &["for_statement"],
        ("loop", LangSpec::JavaScript) | ("loop", LangSpec::TypeScript) | ("loop", LangSpec::Tsx)
            => &["for_statement", "for_in_statement", "for_of_statement", "while_statement", "do_statement"],

        ("try", LangSpec::Python) => &["try_statement"],
        ("try", LangSpec::JavaScript) | ("try", LangSpec::TypeScript) | ("try", LangSpec::Tsx)
            => &["try_statement"],
        // tree-sitter-rust has no top-level `try_expression` node;
        // `?`-postfix is `try_expression` but it's a unary op without
        // a recognizable scope. Leave unsupported.
        ("try", _) => return None,

        ("return", LangSpec::Rust)   => &["return_expression"],
        ("return", LangSpec::Python) => &["return_statement"],
        ("return", LangSpec::Go)     => &["return_statement"],
        ("return", LangSpec::JavaScript) | ("return", LangSpec::TypeScript) | ("return", LangSpec::Tsx)
            => &["return_statement"],

        ("call", LangSpec::Rust)   => &["call_expression", "macro_invocation"],
        ("call", LangSpec::Python) => &["call"],
        ("call", LangSpec::Go)     => &["call_expression"],
        ("call", LangSpec::JavaScript) | ("call", LangSpec::TypeScript) | ("call", LangSpec::Tsx)
            => &["call_expression"],

        _ => return None,
    };
    Some(set.iter().copied().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast;

    fn parse_root_node<'a>(src: &'a str, lang: LangSpec) -> (tree_sitter::Tree, &'a str) {
        (ast::parse(src, lang).unwrap(), src)
    }

    #[test]
    fn parse_segment_basics() {
        let s = parse_segment("body").unwrap();
        assert_eq!(s.alias, "body");
        assert_eq!(s.occurrence, 1);
        let s = parse_segment("loop#3").unwrap();
        assert_eq!(s.alias, "loop");
        assert_eq!(s.occurrence, 3);
        assert!(parse_segment("garbage").is_none());
        assert!(parse_segment("body#0").is_none());
    }

    #[test]
    fn rust_function_body() {
        let src = "fn alpha() { let x = 1; x + 1 }";
        let (tree, _) = parse_root_node(src, LangSpec::Rust);
        let root = tree.root_node();
        // Locate the function_item node.
        let mut cur = root.walk();
        let fn_node = root.children(&mut cur)
            .find(|n| n.kind() == "function_item")
            .expect("function_item present");
        let segs = vec![parse_segment("body").unwrap()];
        let body = resolve_subpath(fn_node, &segs, LangSpec::Rust).expect("body resolves");
        let text = body.utf8_text(src.as_bytes()).unwrap();
        assert!(text.starts_with('{') && text.ends_with('}'));
    }

    #[test]
    fn typescript_nth_call_in_function() {
        let src = "function alpha() { foo(); bar(); baz(); }";
        let (tree, _) = parse_root_node(src, LangSpec::TypeScript);
        let root = tree.root_node();
        let mut cur = root.walk();
        let fn_node = root.children(&mut cur)
            .find(|n| n.kind() == "function_declaration").unwrap();
        let segs = vec![parse_segment("call#2").unwrap()];
        let call = resolve_subpath(fn_node, &segs, LangSpec::TypeScript).expect("call#2");
        let text = call.utf8_text(src.as_bytes()).unwrap();
        assert!(text.starts_with("bar"), "expected bar(), got {text}");
    }

    #[test]
    fn typescript_when_otherwise() {
        let src = r#"
            function alpha() {
                if (cond()) { foo(); }
                else        { bar(); }
            }
        "#;
        let (tree, _) = parse_root_node(src, LangSpec::TypeScript);
        let root = tree.root_node();
        let mut cur = root.walk();
        let fn_node = root.children(&mut cur)
            .find(|n| n.kind() == "function_declaration").unwrap();
        let segs = vec![parse_segment("when").unwrap(), parse_segment("otherwise").unwrap()];
        let alt = resolve_subpath(fn_node, &segs, LangSpec::TypeScript).expect("else");
        let text = alt.utf8_text(src.as_bytes()).unwrap();
        assert!(text.contains("bar"), "expected bar branch, got {text}");
    }
}

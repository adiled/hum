//! Outline formatter — turns a flat `[Symbol]` list into the
//! indented, line-annotated view `humfs_read` renders for code files.
//!
//! The flat list is already sorted by start_byte; we use byte
//! containment to derive nesting: if symbol B's range sits inside
//! symbol A's range, B is indented under A.

use crate::ast::Symbol;

pub fn format_symbols(symbols: &[Symbol]) -> String {
    if symbols.is_empty() {
        return "(no symbols detected)".into();
    }
    let mut out = String::new();
    let mut stack: Vec<usize> = Vec::new(); // indices of "open" parents
    for (i, sym) in symbols.iter().enumerate() {
        // Pop stack while current sym is NOT inside the top of stack.
        while let Some(&top) = stack.last() {
            if sym.start_byte < symbols[top].end_byte {
                break;
            }
            stack.pop();
        }
        let depth = stack.len();
        let indent: String = "  ".repeat(depth);
        out.push_str(&format!(
            "{indent}{tag} {name} L{start}-L{end}\n",
            tag = sym.kind.tag(),
            name = sym.name,
            start = sym.start_row,
            end = sym.end_row,
        ));
        stack.push(i);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::SymbolKind;

    #[test]
    fn nested_indents() {
        let syms = vec![
            Symbol { name: "Outer".into(), kind: SymbolKind::Class, start_byte: 0,  end_byte: 100, start_row: 1,  end_row: 10 },
            Symbol { name: "inner".into(), kind: SymbolKind::Method, start_byte: 20, end_byte: 50,  start_row: 3,  end_row: 5 },
            Symbol { name: "Sibling".into(), kind: SymbolKind::Class, start_byte: 110, end_byte: 200, start_row: 12, end_row: 20 },
        ];
        let out = format_symbols(&syms);
        assert!(out.contains("class Outer"));
        assert!(out.contains("  method inner"));
        assert!(out.contains("class Sibling"));
        assert!(!out.contains("  class Sibling"), "Sibling indented? {}", out);
    }
}

//! `humfs_read` — the ONE filesystem analysis tool. P0 stub.
//!
//! Final shape: unified discovery / study / search (replaces
//! Anthropic's Read+Glob+Grep). path = file | dir | glob (auto-
//! detected by `*` or `?`). Mutually exclusive modifiers: `symbol`
//! (exact), `query` (fuzzy on symbol names), `pattern` (regex over
//! content with enclosing-symbol annotation for code). No offset,
//! no limit, no pagination — by design.
//!
//! P0 only declares the schema + returns a stub. Implementation
//! lands across P2 (no-AST mode) and P4 (AST modifiers).

use nest_common::{ToolDef, ToolResult};
use serde_json::{json, Value};

pub fn def() -> ToolDef {
    ToolDef {
        name: "humfs_read".into(),
        description: "The ONE filesystem analysis tool — discover, study, and search. Works on any file: code returns tree-sitter symbol outline; configs/docs return anchor outline; extensionless and unknown extensions return content. Never refuses based on extension. Path auto-detection: file | directory | glob (presence of * or ?). Pick at most one modifier: symbol (exact AST symbol, dot-nested for nested members), query (fuzzy case-insensitive substring match on symbol NAMES), pattern (regex over CONTENT — code matches carry enclosing symbol). No offset, no limit, no pagination.".into(),
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

pub async fn run(_args: Value) -> ToolResult {
    ToolResult::error("humfs_read: not yet implemented (P0 skeleton; lands in P2/P4)")
}

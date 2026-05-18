//! `humfs_do_noncode` — linguistic-scope edits for non-code files.
//! P0 stub.
//!
//! Four scopes: `word` (single token swap, format-agnostic),
//! `phrase` (structural name OR exact text, format-aware: JSON/YAML
//! key, env var, markdown heading, TOML section), `sentence`
//! (smallest independent unit — JSON comma-entry, YAML sibling
//! key, single line), `paragraph` (full block — enclosing {}/[], YAML
//! indentation block, [section], blank-line para). Omit `replace` to
//! delete; no scope param = whole-file create/overwrite.
//!
//! Refuses code extensions → caller routes to `humfs_do_code`.
//!
//! Implementation lands in P7.

use nest_common::{ToolDef, ToolResult};
use serde_json::{json, Value};

pub fn def() -> ToolDef {
    ToolDef {
        name: "humfs_do_noncode".into(),
        description: "Author non-code files using linguistic scope. Four scopes (pass exactly one): word (format-agnostic token swap), phrase (structural name — JSON/YAML key, env var, markdown heading, TOML section — or exact text), sentence (smallest independent unit), paragraph (full block). Omit 'replace' to delete the scope; no scope param creates/overwrites the whole file. Handles configs, docs, markup, stylesheets, data, plain text. Code files route to humfs_do_code.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "Absolute path to a non-code file." },
                "word":      { "type": "string", "description": "A single token to find and swap. Bounded by spaces/punctuation." },
                "phrase":    { "type": "string", "description": "Structural name OR exact text. For keys/headings/env vars: name stays, governed scope is replaced." },
                "sentence":  { "type": "string", "description": "Text within a sentence. Scope expands to the smallest independent unit." },
                "paragraph": { "type": "string", "description": "Text within a paragraph. Scope expands to the full block." },
                "replace":   { "type": "string", "description": "Replacement content. Omit to delete the scope. No scope param = whole-file create/overwrite." },
            },
            "required": ["file_path"],
        }),
    }
}

pub async fn run(_args: Value) -> ToolResult {
    ToolResult::error("humfs_do_noncode: not yet implemented (P0 skeleton; lands in P7)")
}

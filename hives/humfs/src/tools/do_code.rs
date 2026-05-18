//! `humfs_do_code` — AST-grounded code authoring. P0 stub.
//!
//! Operations: create, replace (symbol-scoped OR whole-file),
//! insert_before, insert_after, delete. Synthetic 'imports' symbol
//! addresses the top-of-file import block. Sub-symbol walks
//! (`body`, `when`, `otherwise`, `loop`, `try`, `return`, `call`)
//! disambiguated with `#N`. Vue SFC sub-blocks (script./template./
//! style.).
//!
//! Refuses non-code extensions → caller routes to `humfs_do_noncode`.
//! Syntax-validates before write (per-language tree-sitter parser).
//!
//! Implementation in P3 (AST infra) + P5 (writes) + P6 (sub-symbol).

use nest_common::{ToolDef, ToolResult};
use serde_json::{json, Value};

pub fn def() -> ToolDef {
    ToolDef {
        name: "humfs_do_code".into(),
        description: "Author code — AST-grounded, symbol-scoped. Operations: create | replace (symbol OR whole-file) | insert_before | insert_after | delete. The top-of-file import block is addressable as the synthetic 'imports' symbol. Sub-symbol walks (body/when/otherwise/loop/try/return/call) compose with dots and disambiguate with #N. Languages: ts/tsx/js/jsx/mjs/cjs/py/pyi/go/rs/java/c/cpp/rb/php/cs/sh/vue (AST-backed); kt/swift/scala/lua/svelte/sql (text-only). Non-code files route to humfs_do_noncode.".into(),
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

pub async fn run(_args: Value) -> ToolResult {
    ToolResult::error("humfs_do_code: not yet implemented (P0 skeleton; lands in P3/P5/P6)")
}

//! Edit — exact find/replace with uniqueness check. Mirrors the native
//! Claude Code Edit tool contract: `old_string` must appear exactly
//! once in the file unless `replace_all` is set.

use crate::protocol::{ToolDef, ToolResult};
use crate::session::SessionState;
use crate::tools::fs_util::assert_path;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Deserialize)]
struct Args {
    file_path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

pub fn def() -> ToolDef {
    ToolDef {
        name: "Edit".to_string(),
        description: "Find-and-replace inside a file. `old_string` must appear exactly once unless `replace_all` is true. Refuses if the file does not exist (use Write to create).".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "file_path":   { "type": "string" },
                "old_string":  { "type": "string" },
                "new_string":  { "type": "string" },
                "replace_all": { "type": "boolean", "default": false },
            },
            "required": ["file_path", "old_string", "new_string"],
        }),
    }
}

pub fn run(args: Value, session: &Arc<Mutex<SessionState>>) -> ToolResult {
    let args: Args = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return ToolResult::error(format!("invalid args: {e}")),
    };
    if args.old_string == args.new_string {
        return ToolResult::error("old_string and new_string are identical — no edit to make");
    }
    let snap = session.lock().clone();
    let abs = match assert_path(&args.file_path, &snap) {
        Ok(p) => p,
        Err(e) => return ToolResult::error(e),
    };
    if let Err(e) = snap.check_permission("Edit", abs.to_str()) {
        return ToolResult::error(e);
    }
    let original = match std::fs::read_to_string(&abs) {
        Ok(s) => s,
        Err(e) => return ToolResult::error(format!("read failed: {e}")),
    };

    let occurrences = original.matches(&args.old_string).count();
    if occurrences == 0 {
        return ToolResult::error(format!(
            "old_string not found in {} — Edit refuses to invent",
            abs.display()
        ));
    }
    if occurrences > 1 && !args.replace_all {
        return ToolResult::error(format!(
            "old_string appears {occurrences} times in {} — pass replace_all=true or give more context",
            abs.display()
        ));
    }

    let updated = if args.replace_all {
        original.replace(&args.old_string, &args.new_string)
    } else {
        original.replacen(&args.old_string, &args.new_string, 1)
    };

    if let Err(e) = std::fs::write(&abs, &updated) {
        return ToolResult::error(format!("write failed: {e}"));
    }
    ToolResult::text(format!(
        "Edited {} ({} occurrence{} replaced)",
        abs.display(),
        occurrences,
        if occurrences == 1 { "" } else { "s" }
    ))
}

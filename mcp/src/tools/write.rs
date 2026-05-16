//! Write — overwrite or create a file. Creates parent dirs on demand.

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
    content: String,
}

pub fn def() -> ToolDef {
    ToolDef {
        name: "Write".to_string(),
        description: "Write `content` to `file_path`, overwriting whatever is there. Creates the file and any missing parent directories.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string" },
                "content":   { "type": "string" },
            },
            "required": ["file_path", "content"],
        }),
    }
}

pub fn run(args: Value, session: &Arc<Mutex<SessionState>>) -> ToolResult {
    let args: Args = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return ToolResult::error(format!("invalid args: {e}")),
    };
    let snap = session.lock().clone();
    let abs = match assert_path(&args.file_path, &snap) {
        Ok(p) => p,
        Err(e) => return ToolResult::error(e),
    };
    if let Err(e) = snap.check_permission("Write", abs.to_str()) {
        return ToolResult::error(e);
    }
    if let Some(parent) = abs.parent() {
        if !parent.exists() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return ToolResult::error(format!("mkdir -p failed: {e}"));
            }
        }
    }
    let bytes = args.content.len();
    if let Err(e) = std::fs::write(&abs, args.content) {
        return ToolResult::error(format!("write failed: {e}"));
    }
    ToolResult::text(format!("Wrote {bytes} bytes to {}", abs.display()))
}

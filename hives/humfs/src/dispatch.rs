//! HumfsDispatcher — the per-process tool registry + dispatch.
//!
//! Owns the session-level state (cwd, fs.roots snapshot) and the
//! mapping from `toolName` → tool impl. ToolDispatcher impl plugs
//! straight into `serve_forager`.

use async_trait::async_trait;
use nest_common::{ToolDef, ToolDispatcher, ToolResult};
use serde_json::Value;

use crate::tools::{bash, do_code, do_noncode, read};

pub struct HumfsDispatcher {
    // Future: SessionState here (cwd, fs.roots, permission cache).
}

impl HumfsDispatcher {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl ToolDispatcher for HumfsDispatcher {
    fn tool_defs(&self) -> Vec<ToolDef> {
        vec![
            read::def(),
            do_code::def(),
            do_noncode::def(),
            bash::def(),
        ]
    }

    async fn dispatch(&self, tone: Value) -> ToolResult {
        let tool_name = tone.get("toolName").and_then(Value::as_str).unwrap_or("");
        let args = tone.get("args").cloned().unwrap_or(Value::Null);
        match tool_name {
            "humfs_read"      => read::run(args).await,
            "humfs_do_code"   => do_code::run(args).await,
            "humfs_do_noncode" => do_noncode::run(args).await,
            "humfs_bash"      => bash::run(args).await,
            other => ToolResult::error(format!("humfs: unknown toolName {other:?}")),
        }
    }
}

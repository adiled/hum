//! Native MCP tool implementations.
//!
//! Each file in this module owns one tool: its schema, its
//! description, its execution path. The shared dispatcher
//! [`dispatch_native`] routes by name; the [`native_tool_defs`]
//! function returns the advertised list for `tools/list`.
//!
//! v0 priority — Read, Edit, Write, Bash, Glob, Grep are real; the
//! rest are stubs that return a "TODO" message.

use crate::protocol::{ToolDef, ToolResult};
use crate::registry::PermissionHook;
use crate::session::SessionState;
use parking_lot::Mutex;
use serde_json::Value;
use std::sync::Arc;

pub mod bash;
pub mod edit;
pub mod fs_util;
pub mod glob;
pub mod grep;
pub mod read;
pub mod write;

/// Canonical native tool names — order shown in `tools/list`. Aliases
/// used by the TS surface (`do_code`, `do_noncode`) are NOT advertised
/// here; the v0 server speaks the Read/Edit/Write/Bash/Glob/Grep
/// surface called out in the spec.
pub const NATIVE_TOOL_NAMES: &[&str] = &[
    "Read",
    "Edit",
    "Write",
    "Bash",
    "Glob",
    "Grep",
    "MultiEdit",
    "Apply",
    "TodoWrite",
    "permission_prompt",
];

pub fn native_tool_defs() -> Vec<ToolDef> {
    vec![
        read::def(),
        edit::def(),
        write::def(),
        bash::def(),
        glob::def(),
        grep::def(),
        stub_def("MultiEdit", "Apply many edits to one file in a single call."),
        stub_def("Apply", "Apply a unified-diff patch to one or more files."),
        stub_def("TodoWrite", "Maintain a task list across the turn."),
        stub_def(
            "permission_prompt",
            "Permission-prompt callback for Claude CLI. Routed to the registry's permission hook.",
        ),
    ]
}

pub async fn dispatch_native(
    name: &str,
    args: Value,
    session: &Arc<Mutex<SessionState>>,
    permission_hook: &Mutex<Option<Arc<dyn PermissionHook>>>,
) -> ToolResult {
    match name {
        "Read" => read::run(args, session),
        "Edit" => edit::run(args, session),
        "Write" => write::run(args, session),
        "Bash" => bash::run(args, session).await,
        "Glob" => glob::run(args, session),
        "Grep" => grep::run(args, session),
        "MultiEdit" | "Apply" | "TodoWrite" => ToolResult::text(format!(
            "[mcpd v0: tool '{name}' is not yet implemented]"
        )),
        "permission_prompt" => permission_prompt(args, permission_hook).await,
        _ => ToolResult::error(format!("Unknown native tool: {name}")),
    }
}

fn stub_def(name: &str, description: &str) -> ToolDef {
    ToolDef {
        name: name.to_string(),
        description: description.to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": true,
        }),
    }
}

async fn permission_prompt(
    args: Value,
    hook: &Mutex<Option<Arc<dyn PermissionHook>>>,
) -> ToolResult {
    let tool_name = args.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
    let input = args.get("input").cloned().unwrap_or(Value::Null);
    let h = hook.lock().clone();
    let decision = match h {
        Some(hook) => match hook.ask("", tool_name, input).await {
            Ok(true) => "allow",
            Ok(false) => "deny",
            Err(e) => {
                return ToolResult::error(format!("permission hook failed: {e}"));
            }
        },
        None => "allow", // No hook installed: default-allow keeps the dev loop unblocked.
    };
    ToolResult::text(
        serde_json::json!({ "behavior": decision }).to_string(),
    )
}

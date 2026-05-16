//! Per-session state: cwd, permissions, allowed tools, registered
//! nestler tools, external MCP tool advertisements, visible-tool filter.
//!
//! Sessions are keyed by the `/s/<session_id>` path segment. The
//! registry creates them lazily on first request — callers set up
//! permissions/cwd by reaching through [`Registry::session`].

use crate::protocol::ToolDef;
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct PermissionRule {
    /// Tool name, or "*" for any.
    pub permission: String,
    /// Path or command pattern. "*" = any.
    pub pattern: String,
    /// "allow" or "deny".
    pub action: String,
}

/// Mutable state for a single MCP session. Behind a Mutex in the
/// registry; tools clone the snapshot they need before doing I/O.
#[derive(Debug, Clone, Default)]
pub struct SessionState {
    pub cwd: PathBuf,
    pub permissions: Vec<PermissionRule>,
    /// `None` = all native tools allowed.
    pub allowed_tools: Option<HashSet<String>>,
    /// Nestler-declared tools advertised through this session.
    pub nestler_tools: Vec<ToolDef>,
    /// External MCP tools advertised through this session (forwarded
    /// out — execution is the caller's problem in v0).
    pub external_tools: Vec<ToolDef>,
    /// If `Some`, only these external tool names are advertised.
    pub visible_external: Option<HashSet<String>>,
}

impl SessionState {
    pub fn new(cwd: PathBuf) -> Self {
        Self { cwd, ..Default::default() }
    }

    /// Apply the same allow/deny logic as `checkPermission` in tools.ts.
    /// Returns Err(reason) when denied, Ok(()) when allowed or rule-set
    /// is silent. `target` is the path (Read/Edit/Write/Glob/Grep) or
    /// command (Bash); pass `None` for tools that have no addressable
    /// target.
    pub fn check_permission(&self, tool: &str, target: Option<&str>) -> Result<(), String> {
        if let Some(allowed) = &self.allowed_tools {
            if !allowed.contains(tool) {
                return Err(format!("Tool \"{tool}\" is not allowed in the current agent mode"));
            }
        }
        if self.permissions.is_empty() {
            return Ok(());
        }
        for rule in &self.permissions {
            if rule.permission != tool && rule.permission != "*" {
                continue;
            }
            let matched = match target {
                Some(t) => {
                    let pat = rule.pattern.trim_end_matches("/*");
                    rule.pattern == "*" || t == rule.pattern || t.starts_with(&format!("{pat}/"))
                }
                None => true,
            };
            if !matched { continue; }
            match rule.action.as_str() {
                "deny" => {
                    return Err(match target {
                        Some(t) => format!("Permission denied: {tool} on {t}"),
                        None => format!("Permission denied: {tool}"),
                    });
                }
                "allow" => return Ok(()),
                _ => {}
            }
        }
        Ok(())
    }
}

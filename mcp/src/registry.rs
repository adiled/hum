//! Tool registry and per-session bookkeeping. The HTTP layer hands every
//! request a `&Registry` and a `session_id`; the registry resolves the
//! session, advertises the right tool set, and dispatches calls.
//!
//! Nestler-declared tools and external MCP tools are dispatched via
//! hooks the caller installs; v0 leaves the wiring to whoever owns the
//! thrum / MCP client connections.

use crate::protocol::{ToolDef, ToolResult};
use crate::session::SessionState;
use crate::tools::{self, native_tool_defs, NATIVE_TOOL_NAMES};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Hook for forwarding nestler-declared tool calls out over thrum.
/// Returns the rendered tool result text on success.
#[async_trait]
pub trait NestlerHook: Send + Sync {
    async fn dispatch(
        &self,
        session_id: &str,
        tool: &str,
        args: Value,
    ) -> anyhow::Result<String>;
}

/// Hook for tool calls handled by forager hives attached over thrum.
/// The provider exposes the tool catalogue it can dispatch (queried
/// once per `tools/list`) and runs each call via thrum routing.
///
/// Installed by humd's daemon boot, backed by the live hive manifest
/// registry. The MCP shell consults the provider FIRST on every
/// `tools/call` — when the name is provider-owned, native dispatch
/// is bypassed entirely.
#[async_trait]
pub trait ForagerToolProvider: Send + Sync {
    /// Tool defs the provider can dispatch. Aggregated across every
    /// attached forager hive. Empty when no foragers are advertising
    /// tool surfaces yet.
    fn list_tools(&self) -> Vec<ToolDef>;

    /// Capability categories the registered forager hives claim
    /// (e.g. `["fs"]`). Used by `list_tools` to strip nestler-
    /// declared tools whose names fall in those capabilities' well-
    /// known sets — the protocol-level decision that says "this
    /// surface is owned elsewhere, don't let the agent reach
    /// locally for it."
    fn provides(&self) -> Vec<String> { Vec::new() }

    async fn dispatch(
        &self,
        session_id: &str,
        tool: &str,
        args: Value,
    ) -> anyhow::Result<String>;
}

/// Hook for permission_prompt — Claude CLI's mid-stream permission
/// callback. Caller decides allow/deny.
#[async_trait]
pub trait PermissionHook: Send + Sync {
    async fn ask(
        &self,
        session_id: &str,
        tool: &str,
        input: Value,
    ) -> anyhow::Result<bool>;
}

/// Hook for observation. Fired after every completed tool execution
/// (native or nestler-roundtripped). Informational only — the
/// callback never affects dispatch or response. The host installs
/// one to broadcast `chi:"tool-info"` to thrum observers (drone,
/// dashboards, rich bees).
pub trait ToolInfoHook: Send + Sync {
    fn record(
        &self,
        session_id: &str,
        tool: &str,
        args: Value,
        result: &str,
        source: ToolInfoSource,
    );
}

/// Where a tool was executed — disambiguates `chi:"tool-info"` events.
#[derive(Debug, Clone, Copy)]
pub enum ToolInfoSource {
    /// Hum's own native MCP tool (Read/Write/Edit/Bash/Glob/Grep…).
    Native,
    /// External MCP server (not yet implemented).
    External,
}

/// Shared, cheaply cloneable handle. Internally `Arc<Inner>`.
#[derive(Clone)]
pub struct Registry {
    inner: Arc<Inner>,
}

struct Inner {
    sessions: Mutex<HashMap<String, Arc<Mutex<SessionState>>>>,
    default_cwd: PathBuf,
    nestler: Mutex<Option<Arc<dyn NestlerHook>>>,
    forager: Mutex<Option<Arc<dyn ForagerToolProvider>>>,
    permission: Mutex<Option<Arc<dyn PermissionHook>>>,
    tool_info: Mutex<Option<Arc<dyn ToolInfoHook>>>,
}

impl Registry {
    pub fn new() -> Self {
        let default_cwd = std::env::var("HUM_CWD")
            .ok()
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("/"));
        Self {
            inner: Arc::new(Inner {
                sessions: Mutex::new(HashMap::new()),
                default_cwd,
                nestler: Mutex::new(None),
                forager: Mutex::new(None),
                permission: Mutex::new(None),
                tool_info: Mutex::new(None),
            }),
        }
    }

    pub fn set_tool_info_hook(&self, h: Arc<dyn ToolInfoHook>) {
        *self.inner.tool_info.lock() = Some(h);
    }

    pub fn set_default_cwd(&self, cwd: PathBuf) {
        // No interior mutability needed — we only consult this on
        // session creation; existing sessions keep what they got.
        if let Ok(inner) = Arc::try_unwrap(self.inner.clone()) {
            let _ = inner; // not actually mutable through Arc; placeholder for API symmetry.
        }
        // Real implementation: stash the default_cwd in a Mutex.
        // Keeping it immutable for v0 — call set_session_cwd instead.
        let _ = cwd;
    }

    pub fn set_nestler_hook(&self, h: Arc<dyn NestlerHook>) {
        *self.inner.nestler.lock() = Some(h);
    }
    pub fn set_forager_provider(&self, h: Arc<dyn ForagerToolProvider>) {
        *self.inner.forager.lock() = Some(h);
    }
    pub fn set_permission_hook(&self, h: Arc<dyn PermissionHook>) {
        *self.inner.permission.lock() = Some(h);
    }

    /// Get-or-create a session handle.
    pub fn session(&self, session_id: &str) -> Arc<Mutex<SessionState>> {
        let mut sessions = self.inner.sessions.lock();
        sessions
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(SessionState::new(self.inner.default_cwd.clone()))))
            .clone()
    }

    /// Drop a session.
    pub fn drop_session(&self, session_id: &str) {
        self.inner.sessions.lock().remove(session_id);
    }

    /// Tools advertised for `tools/list`. Forager-provided tools come
    /// first when registered — they replace the native fs surface
    /// (Read / Write / Edit / Bash / Glob / Grep / MultiEdit / Apply
    /// / TodoWrite). Forager-less builds keep the native fs surface
    /// as before. Nestler-declared and external tools follow.
    pub fn list_tools(&self, session_id: &str) -> Vec<ToolDef> {
        let sess = self.session(session_id);
        let s = sess.lock();

        let forager_defs: Vec<ToolDef> = self.inner.forager.lock().clone()
            .map(|p| p.list_tools())
            .unwrap_or_default();

        let mut out: Vec<ToolDef> = forager_defs.clone();

        let native: Vec<ToolDef> = native_tool_defs()
            .into_iter()
            .filter(|t| {
                // permission_prompt always lands.
                if t.name == "permission_prompt" { return true; }
                // When a forager has taken over the fs surface, drop
                // native tools with the same kind. Today every native
                // tool that isn't permission_prompt is fs-related.
                if !forager_defs.is_empty() { return false; }
                match &s.allowed_tools {
                    Some(set) => set.contains(&t.name),
                    None => true,
                }
            })
            .collect();
        out.extend(native);

        // Capability-level filter. When any registered forager hive
        // declares `provides: ["<cap>"]`, that surface is owned
        // elsewhere; nestler-declared tools whose names fall in
        // the capability's well-known set get dropped from
        // tools/list. Generic mechanism — humd knows the
        // category-to-name mapping (mcp::capability), but
        // categories themselves are wire-level declarations.
        let mut filter_names: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        if let Some(provider) = self.inner.forager.lock().clone() {
            for cap in provider.provides() {
                if let Some(names) = crate::capability::capability_tools(&cap) {
                    for n in names { filter_names.insert((*n).into()); }
                }
            }
        }
        let nestler_iter = s.nestler_tools.iter().filter(|t| {
            !filter_names.contains(&t.name)
        });
        out.extend(nestler_iter.cloned());
        let ext: Vec<ToolDef> = match &s.visible_external {
            Some(vis) => s.external_tools.iter().filter(|t| vis.contains(&t.name)).cloned().collect(),
            None => s.external_tools.clone(),
        };
        out.extend(ext);
        out
    }

    /// Dispatch a `tools/call`. Returns either the native result, the
    /// nestler-hook result, or an error when nothing matches and no
    /// external dispatcher is wired (external MCP execution is out of
    /// scope for v0).
    pub async fn call_tool(
        &self,
        session_id: &str,
        name: &str,
        args: Value,
    ) -> ToolResult {
        // Forager-provided first — when a hive's catalogue contains
        // this name, route via the provider regardless of native /
        // nestler shadowing. The provider checks its own catalogue.
        // Lock-then-clone into a local so the MutexGuard drops
        // BEFORE the await below (axum handler bound is Send).
        let forager_provider = self.inner.forager.lock().clone();
        if let Some(provider) = forager_provider {
            let in_catalogue = provider.list_tools().iter().any(|t| t.name == name);
            if in_catalogue {
                let args_for_hook = args.clone();
                let result = match provider.dispatch(session_id, name, args).await {
                    Ok(text) => ToolResult::text(text),
                    Err(e) => ToolResult::error(format!("forager hive dispatch failed: {e}")),
                };
                let info_hook = self.inner.tool_info.lock().clone();
                if let Some(hook) = info_hook {
                    hook.record(session_id, name, args_for_hook, &result.output, ToolInfoSource::Native);
                }
                return result;
            }
        }

        // Native fallback — only when no forager owns the name.
        if NATIVE_TOOL_NAMES.contains(&name) {
            let sess = self.session(session_id);
            let args_for_hook = args.clone();
            let result = tools::dispatch_native(name, args, &sess, &self.inner.permission).await;
            // Fire the observation hook with the rendered result text
            // so observers can see {sid, name, args, result} as a
            // single chi:"tool-info" event.
            if let Some(hook) = self.inner.tool_info.lock().clone() {
                hook.record(session_id, name, args_for_hook, &result.output, ToolInfoSource::Native);
            }
            return result;
        }

        // Nestler-declared — forward over the hook.
        let is_nestler = {
            let s = self.session(session_id);
            let g = s.lock();
            g.nestler_tools.iter().any(|t| t.name == name)
        };
        if is_nestler {
            let hook = self.inner.nestler.lock().clone();
            return match hook {
                Some(h) => match h.dispatch(session_id, name, args).await {
                    Ok(text) => ToolResult::text(text),
                    Err(e) => ToolResult::error(format!("nestler hook failed: {e}")),
                },
                None => ToolResult::error(format!(
                    "Nestler tool '{name}' advertised but no dispatch hook installed"
                )),
            };
        }

        // External MCP — v0 stub.
        let is_external = {
            let s = self.session(session_id);
            let g = s.lock();
            g.external_tools.iter().any(|t| t.name == name)
        };
        if is_external {
            return ToolResult::error(format!(
                "External MCP tool '{name}' execution is not yet implemented in mcpd v0"
            ));
        }

        ToolResult::error(format!("Unknown tool: {name}"))
    }

}

impl Default for Registry {
    fn default() -> Self { Self::new() }
}

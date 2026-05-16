//! hum's MCP server — HTTP JSON-RPC.
//!
//! The wire shape is one POST per JSON-RPC frame at `/s/<session_id>`,
//! matching what `mcp/tools.ts` + the TS daemon expose today. The session
//! id segment scopes per-session state: cwd, permissions, allowed-tool
//! set, nestler-declared tools, visible external tool filter.
//!
//! Native tools live under `tools::*` and dispatch through [`Registry`].
//! Nestler-declared and external-MCP tools are out-of-process: the
//! registry holds hooks that a caller wires before bringing the server
//! up. v0 ships native correctness for Read / Edit / Write / Bash /
//! Glob / Grep; MultiEdit / Apply / TodoWrite are stubs.
//!
//! ```no_run
//! use mcpd::{Registry, serve};
//! # async fn run() -> anyhow::Result<()> {
//! let registry = Registry::new();
//! serve("127.0.0.1:7777".parse().unwrap(), registry).await?;
//! # Ok(()) }
//! ```

pub mod protocol;
pub mod registry;
pub mod server;
pub mod session;
pub mod tools;

pub use protocol::{JsonRpcError, JsonRpcRequest, JsonRpcResponse, ToolDef, ToolResult};
pub use registry::{NestlerHook, PermissionHook, Registry};
pub use server::serve;
pub use session::{PermissionRule, SessionState};

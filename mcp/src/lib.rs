//! MCP ↔ thrum translation library.
//!
//! Pure data-mapping crate. **No server**, no Axum, no session
//! storage, no Registry. The compute side (each worker bee) decides
//! whether to expose an MCP server at all — if it does, it spawns
//! its own listener and uses these helpers to translate between
//! JSON-RPC frames and `chi:"tool-call"` / `chi:"tool-result"`
//! tones.
//!
//! Modules:
//!
//! - [`protocol`] — JSON-RPC envelope + `ToolDef` / `ToolResult`
//!   shapes
//! - [`capability`] — capability category → tool name set table
//!   (e.g. `"fs"` → `["Read","Write","Edit",…]`)
//! - [`translate`] — `tone ↔ JSON-RPC` shape mapping helpers
//! - [`catalogue`] — merge + filter helpers for `tools/list`
//!   composition

pub mod capability;
pub mod catalogue;
pub mod protocol;
pub mod translate;

pub use protocol::{
    JsonRpcError, JsonRpcRequest, JsonRpcResponse, ToolDef, ToolResult,
};

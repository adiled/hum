//! Worker-side MCP bridge — now a leaf crate.
//!
//! The axum JSON-RPC bridge (spawn_local_mcp / McpBridge / handle)
//! lives in the standalone `hum-mcp` crate, so remote worker hives can
//! spawn an MCP server without pulling the daemon tree. This module is
//! a thin re-export shim for back-compat: existing
//! `nest_common::spawn_local_mcp` / `nest_common::McpBridge` call
//! sites keep resolving.

pub use hum_mcp::{spawn_local_mcp, McpBridge};

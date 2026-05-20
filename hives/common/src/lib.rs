//! `nest-common` — shared building blocks for nest implementations.
//!
//! Today: the regex-driven [`RegexClassifier`] that implements
//! [`drone::Classifier`] for chat-LLM context-loss detection. The
//! [`drone`] crate itself is now a pure primitive — it knows nothing
//! about LLM behavior. Patterns specific to "this looks like a
//! transformer that forgot the conversation" live here.
//!
//! Future additions: shared spawn glue, PTY brood helpers, common
//! Chi handlers, etc. — anything more than one nest crate needs.

pub mod forager;
pub mod identity;
pub mod mcp_bridge;
pub mod serve;
pub mod suspicion_regex;
pub use forager::{serve_forager, ForagerAdvert, ToolDef, ToolDispatcher, ToolResult};
pub use identity::{bee_key_path, load_or_mint_bee_key, BeeKey};
pub use mcp_bridge::{spawn_local_mcp, McpBridge};
pub use serve::{serve_worker, HiveAdvert};
pub use suspicion_regex::RegexClassifier;

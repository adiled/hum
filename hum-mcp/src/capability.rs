//! Capability categories — coarse-grained tool-surface tags a
//! forager hive can claim ownership of.
//!
//! When a hive's `chi:"hello"` declares `provides: ["<cap>"]` (or
//! the daemon detects it via local registration), humd
//! deauthorizes other sources for that surface. Concretely, MCP's
//! `tools/list` filters nestler-declared tools whose names match
//! the capability's well-known set.
//!
//! Today the only category is `"fs"` (filesystem). Future tags
//! (`"net"`, `"shell"`, `"todo"`, …) plug in by adding a row to
//! [`capability_tools`]. The mapping is a hum convention, not a
//! wire negotiation — the asking nestler doesn't ship a category
//! → name table; humd holds it.

/// Tool names mapped to the well-known capability tag they sit
/// inside. Returns the canonical name set for the category, or
/// `None` for an unknown capability.
pub fn capability_tools(cap: &str) -> Option<&'static [&'static str]> {
    match cap {
        "fs" => Some(&[
            "Read", "Write", "Edit", "MultiEdit",
            "Glob", "Grep", "Bash",
        ]),
        _ => None,
    }
}

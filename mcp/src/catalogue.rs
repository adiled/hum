//! Catalogue composition for `tools/list`.
//!
//! Each worker bee gathers two streams of tools:
//!
//! - **forager-provided** (from `chi:"prompt".tools` after humd
//!   merged the manifest's forager hives) — the canonical surfaces
//!   for any capabilities a hive owns
//! - **nestler-provided** (from `chi:"prompt".tools` too, but
//!   shipped by the asking nestler) — the asker's local catalogue
//!
//! When a capability is owned by a forager hive (the forager
//! declared `provides: ["<cap>"]`), the nestler-provided tools
//! whose names fall in that capability's well-known set get
//! filtered out — the agent is supposed to use the forager's
//! surface, not the asker's local fallback.

use crate::capability::capability_tools;
use crate::protocol::ToolDef;

/// Merge + filter the two tool streams into one `tools/list`
/// catalogue. `provided` lists the capability categories some
/// forager hive owns ("fs", future "net" / "shell" / …); names in
/// those capabilities are stripped from `nestler_tools` so the
/// agent picks the forager surface.
///
/// Filter is case-insensitive — askers ship tool names in whatever
/// case they prefer (PascalCase, lowercase, snake_case).
pub fn merge(
    forager_tools: Vec<ToolDef>,
    nestler_tools: Vec<ToolDef>,
    provided: &[String],
) -> Vec<ToolDef> {
    let mut filter: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for cap in provided {
        if let Some(names) = capability_tools(cap) {
            for n in names {
                filter.insert(n.to_ascii_lowercase());
            }
        }
    }

    let mut out: Vec<ToolDef> = forager_tools;
    for t in nestler_tools {
        if filter.contains(&t.name.to_ascii_lowercase()) {
            continue;
        }
        out.push(t);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn def(name: &str) -> ToolDef {
        ToolDef { name: name.into(), description: String::new(), input_schema: json!({}) }
    }

    #[test]
    fn no_capabilities_passes_everything() {
        let merged = merge(
            vec![def("humfs_read")],
            vec![def("Read"), def("MyTodo")],
            &[],
        );
        assert_eq!(merged.len(), 3);
    }

    #[test]
    fn fs_capability_strips_fs_named_nestlers() {
        let merged = merge(
            vec![def("humfs_read"), def("humfs_do_code")],
            vec![def("Read"), def("Write"), def("MyTodo")],
            &["fs".into()],
        );
        let names: Vec<&str> = merged.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"humfs_read"));
        assert!(names.contains(&"humfs_do_code"));
        assert!(names.contains(&"MyTodo"), "non-fs nestler kept: {names:?}");
        assert!(!names.contains(&"Read"), "fs nestler stripped: {names:?}");
        assert!(!names.contains(&"Write"));
    }

    #[test]
    fn fs_capability_filter_is_case_insensitive() {
        let merged = merge(
            vec![def("humfs_read")],
            vec![def("read"), def("write"), def("WRITE"), def("MultiEdit")],
            &["fs".into()],
        );
        let names: Vec<&str> = merged.iter().map(|t| t.name.as_str()).collect();
        assert!(!names.iter().any(|n| n.eq_ignore_ascii_case("read")));
        assert!(!names.iter().any(|n| n.eq_ignore_ascii_case("write")));
        assert!(!names.iter().any(|n| n.eq_ignore_ascii_case("multiedit")));
        assert!(names.contains(&"humfs_read"));
    }

    #[test]
    fn unknown_capability_filters_nothing() {
        let merged = merge(
            vec![],
            vec![def("Read")],
            &["unknown_cap".into()],
        );
        assert_eq!(merged.len(), 1);
    }
}

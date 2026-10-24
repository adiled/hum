//! The hives installed on this humd.
//!
//! A bee is anything that says hello, but a humd understands only the
//! kinds it has been taught, so a hello naming any other hive is refused
//! and a remote bee of an unknown kind is never routed to.
//!
//! A hive once seen is remembered and only a miss looks at disk, so a
//! hive installed while humd runs is recognised without a restart, while
//! forgetting one takes a restart — the bargain the revocation list makes.

use std::collections::HashSet;
use std::sync::Arc;

use parking_lot::RwLock;
use serde_json::Value;

#[derive(Clone)]
pub struct InstalledHives {
    known: Arc<RwLock<HashSet<String>>>,
    looks_on_disk: bool,
}

impl InstalledHives {
    pub fn on_disk() -> Self {
        Self {
            known: Arc::default(),
            looks_on_disk: true,
        }
    }

    pub fn in_memory<I, S>(hives: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            known: Arc::new(RwLock::new(hives.into_iter().map(Into::into).collect())),
            looks_on_disk: false,
        }
    }

    pub fn install(&self, hive: &str) {
        if hum_paths::is_plain_name(hive) {
            self.known.write().insert(hive.to_string());
        }
    }

    pub fn recognises(&self, hive: &str) -> bool {
        if !hum_paths::is_plain_name(hive) {
            return false;
        }
        if self.known.read().contains(hive) {
            return true;
        }
        if self.looks_on_disk && hum_paths::hive_installed(hive) {
            self.install(hive);
            return true;
        }
        false
    }
}

pub fn hive_of(hello: &Value) -> &str {
    hello.get("hive").and_then(Value::as_str).unwrap_or("")
}

pub fn roles_of(hello: &Value) -> Vec<String> {
    match hello.get("roles").or_else(|| hello.get("bee")) {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_an_installed_hive_is_recognised() {
        let hives = InstalledHives::in_memory(["humfs"]);
        assert!(hives.recognises("humfs"));
        assert!(!hives.recognises("claude-cli"));
        hives.install("claude-cli");
        assert!(hives.recognises("claude-cli"));
    }

    #[test]
    fn a_hive_name_that_is_a_path_is_never_recognised() {
        let hives = InstalledHives::in_memory(["humfs"]);
        assert!(!hives.recognises(""));
        assert!(!hives.recognises("../humfs"));
        assert!(!hives.recognises(".humfs"));
        hives.install("../etc");
        assert!(!hives.recognises("../etc"));
    }

    #[test]
    fn roles_are_read_from_roles_then_from_the_old_bee_field() {
        assert_eq!(roles_of(&json!({"roles": ["worker", "forager"]})), ["worker", "forager"]);
        assert_eq!(roles_of(&json!({"bee": ["forager"]})), ["forager"]);
        assert_eq!(roles_of(&json!({"bee": "worker"})), ["worker"]);
        assert_eq!(roles_of(&json!({"roles": ["worker"], "bee": ["forager"]})), ["worker"]);
        assert!(roles_of(&json!({})).is_empty());
    }
}

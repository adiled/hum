//! The hives installed on this humd.
//!
//! A bee is anything that says hello, but a humd understands only the
//! kinds it has been taught, so a hello naming any other hive is refused
//! and a remote bee of an unknown kind is never routed to.
//!
//! A hive once seen is remembered and only a miss looks at disk, so a
//! hive installed while humd runs is recognised without a restart, while
//! forgetting one takes a restart, the bargain the revocation list makes.

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

pub fn headroom_of(tone: &Value) -> Option<ensemble::headroom::CellHeadroom> {
    let claim: thrum_core::Headroom = serde_json::from_value(tone.get("headroom")?.clone()).ok()?;
    Some(ensemble::headroom::CellHeadroom::from_counts(
        claim.free_slots,
        claim.total_slots,
        None,
    ))
}

pub fn role_of(hello: &Value) -> Option<thrum_core::Role> {
    thrum_core::Role::parse(hello.get("role")?.as_str()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_an_installed_hive_is_recognised() {
        let hives = InstalledHives::in_memory(["bash"]);
        assert!(hives.recognises("bash"));
        assert!(!hives.recognises("claude-cli"));
        hives.install("claude-cli");
        assert!(hives.recognises("claude-cli"));
    }

    #[test]
    fn a_hive_name_that_is_a_path_is_never_recognised() {
        let hives = InstalledHives::in_memory(["bash"]);
        assert!(!hives.recognises(""));
        assert!(!hives.recognises("../bash"));
        assert!(!hives.recognises(".bash"));
        hives.install("../etc");
        assert!(!hives.recognises("../etc"));
    }

    #[test]
    fn headroom_is_read_from_a_claim() {
        let full = headroom_of(&json!({"headroom": {"freeSlots": 0, "totalSlots": 8}})).unwrap();
        assert!(full.pressure.is_refusing());
        let room = headroom_of(&json!({"headroom": {"freeSlots": 6, "totalSlots": 8}})).unwrap();
        assert!(!room.pressure.is_refusing());
        assert!(headroom_of(&json!({})).is_none());
        assert!(headroom_of(&json!({"headroom": {"free": 1}})).is_none());
    }

    #[test]
    fn a_role_is_one_of_four() {
        assert_eq!(role_of(&json!({"role": "nurse"})), Some(thrum_core::Role::Nurse));
        assert_eq!(role_of(&json!({"role": "builder"})), Some(thrum_core::Role::Builder));
        assert_eq!(role_of(&json!({"role": "worker"})), None);
        assert_eq!(role_of(&json!({"roles": ["forager"]})), None);
        assert_eq!(role_of(&json!({})), None);
    }
}

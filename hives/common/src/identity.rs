//! Bee identity — now a leaf crate.
//!
//! The persistent-identity logic (load-or-mint an Ed25519 seed at
//! `$XDG_STATE_HOME/hum/bees/<kind>.key`, derive the role-tagged
//! [`Hid`]) lives in the standalone `hum-identity` crate, so remote
//! hives can depend on it without pulling in the daemon tree.
//!
//! This module is a thin re-export shim for back-compat during the
//! transition: existing `nest_common::load_or_mint_bee_key` /
//! `nest_common::BeeKey` / `nest_common::bee_key_path` call sites keep
//! resolving. New hives should depend on `hum-identity` directly.

pub use hum_identity::{bee_key_path, load_or_mint_bee_key, BeeKey};

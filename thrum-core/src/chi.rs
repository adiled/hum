//! Chi registry, pulse kinds, petal kinds, roles and advisory codes.
//!
//! Every wire-known chi value. Adding a new variant bumps the protocol
//! minor version. `serde(rename_all = "kebab-case")` keeps the wire
//! format speaking kebabs (`"tendril-reach"`, `"drone-retrofit"`).
//!
//! The thrum, which chi exists, what it carries, who sounds it and who
//! hears it, is `hum.thrum` at the repo root. These enums are its Rust
//! reading, and `cargo run -p codegen -- --check` holds them to it,
//! value for value; the TypeScript, Python, Go and C clients are
//! emitted from the same file, so no copy can drift.
//!
//! A petal is one unit of content, and its kind says which:
//! [`PetalKind`]. `KIND petal-kind` says what each one carries.
//!
//! A bee does one [`Role`], set when its hive enters it, and its hello
//! says which. Each chi's EMITS and HEARS say what a role sends and hears.
//!
//! An advisory is what the wire tells a bee it should fix without
//! refusing it: humd sends one as a `log` tone whose `fields.advisory`
//! is an [`AdvisoryCode`], and the client raises its own under the same
//! codes. Each ADVISORY says what it means and who raises it.

use serde::{Deserialize, Serialize};
use strum::{EnumIter, IntoStaticStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, EnumIter, IntoStaticStr)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Chi {
    Hello,
    Prompt,
    Cancel,
    Cleanup,
    Curate,
    ScoutResult,
    TendrilResult,
    Breath,
    Unfurl,
    Wilt,
    Error,
    Pulse,
    Scout,
    TendrilReach,
    TendrilMeta,
    TendrilInfo,
    Echo,
    PerfMark,
    Log,
    Drone,
    DroneRetrofit,
    PeerAdd,
    PeerRemove,
    Attach,
    Detach,
    WaneSync,
    GossipPublish,
    KadFindNode,
    KadFindNodeResp,
    Backfill,
    BackfillEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, EnumIter, IntoStaticStr)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum PulseKind {
    CellSpawned,
    CellReady,
    CellIdle,
    CellDied,
    CellEvicted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, EnumIter, IntoStaticStr)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum AdvisoryCode {
    UndeclaredKey,
    UnknownChi,
    NotATone,
    ClientOwned,
    ToneRefused,
    DroppedWhileAway,
    ProtoDrift,
    ReconnectStorm,
    DidNotReturn,
    CwdNotHum,
    Refused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, EnumIter, IntoStaticStr)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum PetalKind {
    Text,
    Tendril,
    Grasp,
    Pollen,
    Sepal,
    Waggle,
    Thorn,
    Brix,
    Bouquet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, EnumIter, IntoStaticStr)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Role {
    Forager,
    Nurse,
    Builder,
    Attendant,
}

impl Chi {
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

impl Role {
    pub fn as_str(self) -> &'static str {
        self.into()
    }

    pub fn parse(s: &str) -> Option<Role> {
        serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
    }
}

impl PetalKind {
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

impl AdvisoryCode {
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

pub fn is_chi(s: &str) -> bool {
    serde_json::from_value::<Chi>(serde_json::Value::String(s.to_string())).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chi_wire_format_is_kebab() {
        assert_eq!(serde_json::to_string(&Chi::TendrilReach).unwrap(), "\"tendril-reach\"");
        assert_eq!(serde_json::to_string(&Chi::Unfurl).unwrap(), "\"unfurl\"");
        assert_eq!(serde_json::to_string(&Chi::Wilt).unwrap(), "\"wilt\"");
        assert_eq!(PetalKind::Sepal.as_str(), "sepal");
        assert_eq!(serde_json::to_string(&Chi::DroneRetrofit).unwrap(), "\"drone-retrofit\"");
        assert_eq!(serde_json::to_string(&Chi::WaneSync).unwrap(), "\"wane-sync\"");
        let parsed: Chi = serde_json::from_str("\"wane-sync\"").unwrap();
        assert_eq!(parsed, Chi::WaneSync);
    }

    #[test]
    fn advisory_codes_are_kebab_and_chi_is_checked() {
        assert_eq!(AdvisoryCode::UndeclaredKey.as_str(), "undeclared-key");
        assert_eq!(serde_json::to_string(&AdvisoryCode::DidNotReturn).unwrap(), "\"did-not-return\"");
        assert!(is_chi("tendril-reach"));
        assert!(!is_chi("tendril_reach"));
        assert!(!is_chi("tool-call"));
    }

    #[test]
    fn a_role_is_one_of_four_and_kebab() {
        assert_eq!(Role::Nurse.as_str(), "nurse");
        assert_eq!(Role::parse("builder"), Some(Role::Builder));
        assert_eq!(Role::parse("attendant"), Some(Role::Attendant));
        assert_eq!(Role::parse("worker"), None);
        assert_eq!(AdvisoryCode::CwdNotHum.as_str(), "cwd-not-hum");
    }

    #[test]
    fn pulse_kind_wire_format_is_kebab() {
        assert_eq!(serde_json::to_string(&PulseKind::CellEvicted).unwrap(), "\"cell-evicted\"");
        let parsed: PulseKind = serde_json::from_str("\"cell-ready\"").unwrap();
        assert_eq!(parsed, PulseKind::CellReady);
    }
}

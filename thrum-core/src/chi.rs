//! Chi registry and pulse kinds.
//!
//! Every wire-known chi value. Adding a new variant bumps the protocol
//! minor version. `serde(rename_all = "kebab-case")` keeps the wire
//! format speaking kebabs (`"tool-call"`, `"release-permit"`).
//!
//! The wire contract — which chi exists, what it carries, and which way
//! it flows — lives in [`WIRE.md`](../../WIRE.md). This enum is its
//! Rust mirror; the generated TypeScript, Python, and Go clients are
//! emitted from that same contract, so the four copies cannot drift.

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
    ReleasePermit,
    TendrilResult,
    ToolResult,
    PetalCell,
    Breath,
    Chunk,
    Finish,
    Error,
    SessionReady,
    Pulse,
    PermissionAsk,
    TendrilReach,
    ToolCall,
    ToolMeta,
    ToolInfo,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chi_wire_format_is_kebab() {
        assert_eq!(serde_json::to_string(&Chi::ToolCall).unwrap(), "\"tool-call\"");
        assert_eq!(serde_json::to_string(&Chi::ReleasePermit).unwrap(), "\"release-permit\"");
        assert_eq!(serde_json::to_string(&Chi::DroneRetrofit).unwrap(), "\"drone-retrofit\"");
        assert_eq!(serde_json::to_string(&Chi::WaneSync).unwrap(), "\"wane-sync\"");
        let parsed: Chi = serde_json::from_str("\"petal-cell\"").unwrap();
        assert_eq!(parsed, Chi::PetalCell);
        let parsed: Chi = serde_json::from_str("\"wane-sync\"").unwrap();
        assert_eq!(parsed, Chi::WaneSync);
    }

    #[test]
    fn pulse_kind_wire_format_is_kebab() {
        assert_eq!(serde_json::to_string(&PulseKind::CellEvicted).unwrap(), "\"cell-evicted\"");
        let parsed: PulseKind = serde_json::from_str("\"cell-ready\"").unwrap();
        assert_eq!(parsed, PulseKind::CellReady);
    }
}

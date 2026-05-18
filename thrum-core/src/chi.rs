//! Chi registry and pulse kinds.
//!
//! Every wire-known chi value. Adding a new variant bumps the protocol
//! minor version. `serde(rename_all = "kebab-case")` keeps the wire
//! format speaking kebabs (`"tool-call"`, `"release-permit"`).

use serde::{Deserialize, Serialize};
use strum::{EnumIter, IntoStaticStr};

/// Discriminator for every thrum frame.
///
/// Direction is encoded in the docstring, not the type — the same socket
/// carries both directions and either end may legally send any chi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, EnumIter, IntoStaticStr)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Chi {
    // ── Nestler → Daemon ────────────────────────────────────────────
    /// announce self — protoVersion, bee, version
    Hello,
    /// start a turn — content, system, tools
    Prompt,
    /// interrupt mid-turn
    Cancel,
    /// session deleted, drop daemon state
    Cleanup,
    /// manual compaction request
    Curate,
    /// resolve an earlier permission-ask
    ReleasePermit,
    /// task subagent answered
    TendrilResult,
    /// nestler-declared tool answered
    ToolResult,
    /// OC message-graph update (graft hint)
    PetalCell,

    // ── Daemon → Nestler ────────────────────────────────────────────
    /// handshake — full state sync on connect
    Breath,
    /// model output partwise (text/reasoning/tool)
    Chunk,
    /// turn complete — finishReason + usage
    Finish,
    /// turn aborted
    Error,
    /// nest spawned, claude session id known
    SessionReady,
    /// process lifecycle event
    Pulse,
    /// mid-stream permission needed
    PermissionAsk,
    /// task subagent dispatch
    TendrilReach,
    /// nestler-declared tool dispatch
    ToolCall,
    /// out-of-band metadata for a tool result
    ToolMeta,
    /// completed-in-server tool execution — purely informational.
    /// Carries the full {callId, name, args, result} pair as one
    /// tone. Unlike `ToolCall`/`ToolResult` (which form a two-phase
    /// handoff requiring the recipient to execute and reply),
    /// `ToolInfo` says "this tool ran upstream, here's everything."
    /// Consumers render it without taking action. Used by humd when
    /// its native MCP tools run in-process, or when a nestler-tool
    /// roundtrip completes successfully and observers want a clean
    /// audit event.
    ToolInfo,

    // ── Either direction ────────────────────────────────────────────
    /// delivery ack for a rid
    Echo,
    /// drift timing — measured both ways
    PerfMark,
    /// structured log forwarding
    Log,
    /// drone heartbeat
    Drone,
    /// drone swallow + retry signal
    DroneRetrofit,

    // ── Ensemble / inter-humd plumbing ──────────────────────────────
    /// register a peer humd — `{ humd_id: hex, hints: [..] }`
    PeerAdd,
    /// drop a peer humd — `{ humd_id: hex }`
    PeerRemove,
    /// peer humd asks to observe a sid hosted here — `{ sid, to, from, hearOnly }`.
    /// The host records `from` as an observer of `sid`; reply tones for
    /// that sid fan out to every observer in addition to the prompt origin.
    Attach,
    /// peer humd stops observing — `{ sid, to, from }`. The host drops
    /// `from` from the observer roster for `sid`.
    Detach,
    /// reconcile WaneTracker after a partition heal — `{ from, snapshot }`
    /// where `snapshot` is a map of sigil → wane value. The receiver merges
    /// each entry by taking the max of local and remote (wane is a Lamport
    /// clock; max is convergent). No reply is required — both sides emit
    /// their snapshot on heal, so each is informed exactly once.
    WaneSync,
    /// ensemble-wide gossip pub-sub message —
    /// `{ topic, payload, from, msg_id }`. Fan-out broadcast above the
    /// Transport seam: every PeerConnection is a gossip neighbor. The
    /// receiver dedups on `msg_id` (sha256("topic:rid:from:payload")[..16])
    /// using a bounded LRU, dispatches to per-topic subscribers, and
    /// re-fans the tone to every OTHER installed peer so the message
    /// percolates across the mesh. Distinct from unicast `route` (which
    /// targets ONE humd) — gossip is mesh-wide announcements: hum
    /// relocation, humd overload, drone alerts.
    GossipPublish,
    /// Kademlia DHT FIND_NODE query —
    /// `{ query_id, target: <Hid hex>, from: <Hid hex> }`. The
    /// receiver answers with `kad-find-node-resp` carrying up to K
    /// HumdAddrs from its routing table closest in XOR distance to
    /// `target`. Sent during `Ensemble::kad_find` iterative lookups
    /// when a peer's HumdAddr isn't already known locally.
    KadFindNode,
    /// Kademlia DHT FIND_NODE response —
    /// `{ query_id, from: <Hid hex>, closest: [<HumdAddr JSON>, ...] }`.
    /// Matched to the originating `kad-find-node` by `query_id`. The
    /// lookup driver inserts every advertised HumdAddr into its routing
    /// table and re-queries the α closest unqueried peers until no
    /// closer node is returned.
    KadFindNodeResp,
}

/// `pulse.kind` — its own enum within `chi:"pulse"` tones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, EnumIter, IntoStaticStr)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum PulseKind {
    /// process created
    CellSpawned,
    /// system init received, accepting input
    CellReady,
    /// turn complete, no listeners
    CellIdle,
    /// process exited
    CellDied,
    /// killed to make room
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

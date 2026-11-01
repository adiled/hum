//! `kad` — homegrown Kademlia DHT for Hid → HumdAddr discovery.
//!
//! T4 lookup layer above the [`crate::Transport`] seam, parallel to the
//! gossip fan-out. Goal is the same one Kademlia solves in the paper:
//! given a [`Hid`] you don't already have a connection to, find a
//! [`HumdAddr`] for it by iteratively asking peers progressively closer
//! to the target in XOR space. peers.json gives us the static T1/T2
//! seed list; this gives us the dynamic T4 dial-the-stranger surface.
//!
//! ## Why homegrown vs `libp2p-kad`
//!
//! `libp2p-kad` is a `NetworkBehaviour` glued to `libp2p::Swarm` with
//! its own RPC framing, its own peer-id space, its own iterative-lookup
//! state machine, its own k-bucket layout. Plugging it in means either
//! standing the full Swarm runtime up alongside our `Transport` trait
//! or writing a libp2p-transport shim that smuggles our
//! [`PeerConnection`]s through. Same architectural trade we made in
//! [`crate::gossip`] — two parallel wire abstractions, or a shim that
//! pretends.
//!
//! Instead: ~500 LoC, XOR distance over the 32-byte Hid, a 256-slot
//! routing table (one [`KBucket`] per leading-zero distance bucket), an
//! iterative FIND_NODE that fans out α=3 queries in parallel and
//! terminates when no nearer peer is returned. Two new chi values
//! (`kad-find-node`, `kad-find-node-resp`) carry queries over peer
//! `send()` — exactly the same wire path gossip uses.
//!
//! ## What we DO get
//! - O(log n) lookup in expectation across the mesh.
//! - Bounded-memory routing table (K=20 per bucket × 256 buckets max).
//! - Convergence to closer peers as the lookup progresses.
//!
//! ## What we DON'T get yet
//! - No ping-based liveness eviction — LRU is "last inserted wins."
//! - No periodic bucket refresh, no random-id self-lookups.
//! - No replication / `STORE` — this is pure node lookup (FIND_NODE).
//! - No security: a hostile peer can return arbitrary HumdAddrs and
//!   we trust them. Real DHTs layer S/Kademlia or Coral on top.
//!
//! When the mesh grows past ~thousands of nodes, or we need value
//! storage, swapping in `libp2p-kad` becomes worthwhile. The chi
//! shapes and the [`crate::Ensemble::kad_find`] surface stay stable.
//!
//! Distance is `a ^ b` byte-wise, ordered lexicographically over the
//! resulting 32 bytes, so a lower value means closer. The bucket index
//! is the count of leading zero bits in that distance: `me == peer`
//! yields 256, meaning "no bucket", and callers filter self out first.

mod bucket;
mod distance;
mod lookup;
mod state;
mod table;
mod wire;

pub use bucket::KBucket;
pub use distance::XorDistance;
pub use lookup::KadFindOutcome;
pub use state::KadState;
pub use table::RoutingTable;
pub use wire::{
    ParsedFindNode, ParsedFindNodeResp, find_node_resp_tone, find_node_tone, mint_query_id,
    parse_find_node, parse_find_node_resp,
};

pub(crate) use lookup::{LookupShortlist, query_peer};

pub const KAD_K: usize = 20;

pub const KAD_ALPHA: usize = 3;

pub const KAD_MAX_ROUNDS: usize = 16;

pub const KAD_FIND_NODE_CHI: &str = "kad-find-node";

pub const KAD_FIND_NODE_RESP_CHI: &str = "kad-find-node-resp";

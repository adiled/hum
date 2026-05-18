//! sim — in-process ensemble simulator.
//!
//! Spin up N `humd` instances in one process, wire them with
//! `ensemble::InMemoryEndpoint::pair`, fake-network them, and run
//! narrative tests against the result. No sockets, no real subprocesses,
//! no I/O — every humd's `Thrum` is in-memory and every nest uses
//! `nest::MockWorkerBee`.
//!
//! This crate is the foundation for the narrative test suite. It owns
//! lifecycle (spawn/wire/shutdown) and the synthetic-nestler hooks
//! (`nestler_send`, `nestler_recv`). The tests themselves live in the
//! caller (smoke test here is just a vital-signs check).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use ensemble::{hello_tone, Ensemble, HumdId, HumdKey, InMemoryEndpoint, PeerCapabilities};
use parking_lot::{Mutex, RwLock};
use serde_json::Value;
use thrum_core::WaneTracker;
use thrumd::Thrum;
use tokio::sync::{mpsc, oneshot};

/// Sentinel meaning "unlimited capacity" — round-trips through
/// `set_capacity` / `wire` without overflow concerns and reads as
/// `None` in caps advertised on the wire.
const CAPACITY_UNLIMITED: usize = usize::MAX;

/// One in-process humd inside the sim: its identity, its Thrum (so the
/// sim can drive tones), its Ensemble (so the sim can wire peers), and
/// the shutdown handle for the spawned task.
pub struct SimHumd {
    pub id: HumdId,
    pub thrum: Thrum,
    pub ensemble: Arc<Ensemble>,
    pub shutdown_tx: Mutex<Option<oneshot::Sender<()>>>,
    pub join: Mutex<Option<tokio::task::JoinHandle<Result<()>>>>,
    /// Per-synthetic-client outbound queues. `nestler_recv` pops from
    /// these. Keyed by the synthetic client_id we minted on send.
    /// Each entry is the receiver Thrum hands back from
    /// `register_synthetic`.
    out_queues: Mutex<HashMap<String, mpsc::Receiver<Value>>>,
    /// Per-sid mailbox: any tone whose `sid` matches drops in here.
    /// Built lazily by `nestler_recv`; fed by a fanout task per
    /// synthetic client.
    sid_mailboxes: Mutex<HashMap<String, mpsc::UnboundedReceiver<Value>>>,
    sid_senders: Mutex<HashMap<String, mpsc::UnboundedSender<Value>>>,
    /// Max concurrent local hums this humd will accept. `usize::MAX` ==
    /// unlimited. Read by `wire` to populate the cap advertisement and
    /// by `spawn_humd` to set `DaemonConfig::capacity_override`.
    capacity: AtomicUsize,
    /// Per-humd WaneTracker. Built sim-side so tests can read/write wane
    /// values directly (`sim_humd.waneman.get("sigil")`) and so the
    /// partition-heal reconciliation handshake can snapshot the local
    /// state. Shared with the daemon's HumdSink via
    /// `DaemonConfig::waneman`.
    pub waneman: Arc<WaneTracker>,
    /// Ed25519 signing identity, when the humd was spawned via
    /// [`Sim::spawn_humd_with_identity`]. Federation paths
    /// ([`Sim::wire_signed`]) require this; legacy `spawn_humd(id)` calls
    /// leave it `None` and use unsigned hellos.
    pub key: Option<Arc<HumdKey>>,
}

pub struct Sim {
    humds: RwLock<HashMap<HumdId, Arc<SimHumd>>>,
    /// Capacities set via `set_capacity` BEFORE the humd was spawned.
    /// Drained by `spawn_humd` on entry.
    pending_capacities: RwLock<HashMap<HumdId, usize>>,
    /// In-memory link endpoints, keyed by (lower-id, higher-id). Each
    /// entry holds the two `InMemoryEndpoint` Arcs so `partition` /
    /// `heal` can flip them in both directions. The map is canonicalised
    /// so `partition(a, b)` and `partition(b, a)` reach the same entry.
    links: RwLock<HashMap<(HumdId, HumdId), Link>>,
}

/// One sim-managed link between two humds. `a_end` is the endpoint held
/// by humd `a` (it sends through this to reach `b`); `b_end` is the
/// mirror. To fully partition the link we flip both — each blocks its
/// own outbound side.
struct Link {
    a: HumdId,
    b: HumdId,
    a_end: Arc<InMemoryEndpoint>,
    b_end: Arc<InMemoryEndpoint>,
}

/// Canonical key so (a, b) and (b, a) hash to the same slot.
fn link_key(x: HumdId, y: HumdId) -> (HumdId, HumdId) {
    if x.to_hex() <= y.to_hex() { (x, y) } else { (y, x) }
}

impl Default for Sim {
    fn default() -> Self {
        Self::new()
    }
}

impl Sim {
    pub fn new() -> Self {
        Self {
            humds: RwLock::new(HashMap::new()),
            pending_capacities: RwLock::new(HashMap::new()),
            links: RwLock::new(HashMap::new()),
        }
    }

    /// Cap how many concurrent local hums a humd will host. `0` forces
    /// every prompt to overflow to a peer; `usize::MAX` means unlimited
    /// (the default). Must be called BEFORE the humd is spawned OR after
    /// — if after, the cap takes effect on next prompt but advertised
    /// caps to existing peers are not updated (re-wire to refresh).
    pub fn set_capacity(&self, humd: HumdId, max_concurrent: usize) {
        if let Some(h) = self.humds.read().get(&humd).cloned() {
            h.capacity.store(max_concurrent, Ordering::SeqCst);
            return;
        }
        self.pending_capacities.write().insert(humd, max_concurrent);
    }

    /// Spawn an in-process humd with the given id. Builds a fresh
    /// `Thrum` and `Ensemble`, plugs both into a `humd::DaemonConfig`,
    /// and launches `humd::run` on a task. Returns immediately. If the
    /// caller called [`Sim::set_capacity`] for this id BEFORE the spawn,
    /// the stored value is consumed here and threaded into the daemon
    /// config + the `SimHumd`'s atomic.
    pub async fn spawn_humd(&self, id: HumdId) -> Arc<SimHumd> {
        let thrum = Thrum::new();
        let ensemble = Arc::new(Ensemble::new(id));
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        // Dummy paths — they're inert when bind_mcp=false and thrum_override
        // is supplied, but DaemonConfig wants something to hold.
        let tmp = std::env::temp_dir().join(format!("sim-humd-{}", id.short()));
        let _ = std::fs::create_dir_all(&tmp);
        let penny_path = tmp.join("penny.json");

        // Drain any pre-spawn capacity hint and reuse it for both the
        // daemon's overflow policy AND the SimHumd's published atomic so
        // `wire` and the daemon agree on the same number.
        let initial_capacity = self
            .pending_capacities
            .write()
            .remove(&id)
            .unwrap_or(CAPACITY_UNLIMITED);
        let capacity_override = if initial_capacity == CAPACITY_UNLIMITED {
            None
        } else {
            Some(initial_capacity)
        };

        let waneman = Arc::new(WaneTracker::new());
        let cfg = humd::DaemonConfig {
            thrum_path: tmp.join("thrum.sock"),
            http_path: tmp.join("http.sock"),
            mcp_addr: ([127, 0, 0, 1], 0).into(),
            penny_path,
            hum_cfg: config::HumConfig::default(),
            cli_path: "noop".into(),
            penny_persist_interval: Duration::from_secs(3600),
            thrum_override: Some(thrum.clone()),
            ensemble: Some(ensemble.clone()),
            bind_mcp: false,
            capacity_override,
            waneman: Some(waneman.clone()),
            humd_key: None,
            bootstrap_peers: Vec::new(),
        };

        let shutdown_fut = async move {
            let _ = shutdown_rx.await;
        };
        let join = tokio::spawn(humd::run(cfg, shutdown_fut));

        let sim_humd = Arc::new(SimHumd {
            id,
            thrum,
            ensemble,
            shutdown_tx: Mutex::new(Some(shutdown_tx)),
            join: Mutex::new(Some(join)),
            out_queues: Mutex::new(HashMap::new()),
            sid_mailboxes: Mutex::new(HashMap::new()),
            sid_senders: Mutex::new(HashMap::new()),
            capacity: AtomicUsize::new(initial_capacity),
            waneman,
            key: None,
        });

        self.humds.write().insert(id, sim_humd.clone());
        sim_humd
    }

    /// Wire two humds with an in-memory channel pair. Both ensembles
    /// pick up a `PeerConnection` to the other; capabilities mirror each
    /// side's current capacity — sim humds always claim `claude-cli`
    /// support so overflow routing has somewhere to land, and the
    /// advertised `free_slots` reflects the atomic set via
    /// [`Sim::set_capacity`] (default = unlimited).
    pub fn wire(&self, a: HumdId, b: HumdId) -> Result<()> {
        let humds = self.humds.read();
        let ha = humds
            .get(&a)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", a.short()))?;
        let hb = humds
            .get(&b)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", b.short()))?;
        drop(humds);

        let caps_for = |humd: &SimHumd| {
            let cap = humd.capacity.load(Ordering::SeqCst);
            let free_slots = if cap == CAPACITY_UNLIMITED { None } else { Some(cap) };
            PeerCapabilities {
                proto_version: thrum_core::THRUM_VERSION.to_string(),
                // Match humd's default perch_tag (config::HumConfig::default
                // → nest.default = "claude-repl"). Overflow lookup keys on
                // this nest name; mismatch with the daemon's tag breaks
                // the test deterministically.
                nests: vec!["claude-repl".to_string()],
                free_slots,
                ..Default::default()
            }
        };
        let a_caps = caps_for(&ha);
        let b_caps = caps_for(&hb);
        // Each InMemoryEndpoint stores the *peer*'s transport-claimed
        // caps. So a's view of b carries b_caps; b's view of a carries
        // a_caps. The handshake's `learned_caps` will overwrite this
        // when the hello arrives.
        let (a_view, b_view) = InMemoryEndpoint::pair_concrete(
            ha.id,
            b_caps.clone(),
            hb.id,
            a_caps.clone(),
        );
        // Stash the typed handles before we hand them off as trait objects
        // — `partition` / `heal` flip them through `set_partitioned`.
        let key = link_key(ha.id, hb.id);
        self.links.write().insert(
            key,
            Link {
                a: ha.id,
                b: hb.id,
                a_end: a_view.clone(),
                b_end: b_view.clone(),
            },
        );
        // Unsigned-hello install — each side announces its OWN caps so
        // the other side's `peer_caps` lookup returns the real nests +
        // free_slots. Without this, every sim humd's peers learn nothing
        // and the overflow router has no signal.
        ha.ensemble
            .add_peer_with_caps(a_view as Arc<dyn ensemble::PeerConnection>, a_caps);
        hb.ensemble
            .add_peer_with_caps(b_view as Arc<dyn ensemble::PeerConnection>, b_caps);
        Ok(())
    }

    /// Spawn an in-process humd whose [`HumdId`] is derived from a real
    /// Ed25519 keypair. The id comes from the key — `humd_id =
    /// sha256(pubkey)` is one-way, so the caller can't pin both. Use
    /// this for federation tests where `wire_signed` needs a real key.
    /// The resulting ensemble has `strict_auth=true`, so unsigned or
    /// invalid hellos from peers are rejected.
    pub async fn spawn_humd_with_identity(&self, key: HumdKey) -> Arc<SimHumd> {
        let id = key.humd_id();
        let thrum = Thrum::new();
        let ensemble = Arc::new(Ensemble::with_strict_auth(id, true));
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        let tmp = std::env::temp_dir().join(format!("sim-humd-{}", id.short()));
        let _ = std::fs::create_dir_all(&tmp);
        let penny_path = tmp.join("penny.json");

        let initial_capacity = self
            .pending_capacities
            .write()
            .remove(&id)
            .unwrap_or(CAPACITY_UNLIMITED);
        let capacity_override = if initial_capacity == CAPACITY_UNLIMITED {
            None
        } else {
            Some(initial_capacity)
        };

        let waneman = Arc::new(WaneTracker::new());
        let cfg = humd::DaemonConfig {
            thrum_path: tmp.join("thrum.sock"),
            http_path: tmp.join("http.sock"),
            mcp_addr: ([127, 0, 0, 1], 0).into(),
            penny_path,
            hum_cfg: config::HumConfig::default(),
            cli_path: "noop".into(),
            penny_persist_interval: Duration::from_secs(3600),
            thrum_override: Some(thrum.clone()),
            ensemble: Some(ensemble.clone()),
            bind_mcp: false,
            capacity_override,
            waneman: Some(waneman.clone()),
            humd_key: None,
            bootstrap_peers: Vec::new(),
        };

        let shutdown_fut = async move { let _ = shutdown_rx.await; };
        let join = tokio::spawn(humd::run(cfg, shutdown_fut));

        let sim_humd = Arc::new(SimHumd {
            id,
            thrum,
            ensemble,
            shutdown_tx: Mutex::new(Some(shutdown_tx)),
            join: Mutex::new(Some(join)),
            out_queues: Mutex::new(HashMap::new()),
            sid_mailboxes: Mutex::new(HashMap::new()),
            sid_senders: Mutex::new(HashMap::new()),
            capacity: AtomicUsize::new(initial_capacity),
            waneman,
            key: Some(Arc::new(key)),
        });

        self.humds.write().insert(id, sim_humd.clone());
        sim_humd
    }

    /// Wire two humds with **signed** hellos under strict auth — the
    /// federation path. Both humds must have been spawned via
    /// [`Sim::spawn_humd_with_identity`]. Each side announces a signed
    /// `chi:"hello"` and verifies the other's signature before
    /// admitting the peer.
    pub fn wire_signed(&self, a: HumdId, b: HumdId) -> Result<()> {
        let humds = self.humds.read();
        let ha = humds.get(&a).cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", a.short()))?;
        let hb = humds.get(&b).cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", b.short()))?;
        drop(humds);
        let a_key = ha.key.clone().ok_or_else(|| anyhow::anyhow!(
            "humd {} has no signing key — spawn via spawn_humd_with_identity",
            a.short()
        ))?;
        let b_key = hb.key.clone().ok_or_else(|| anyhow::anyhow!(
            "humd {} has no signing key — spawn via spawn_humd_with_identity",
            b.short()
        ))?;

        let caps_for = |humd: &SimHumd| {
            let cap = humd.capacity.load(Ordering::SeqCst);
            let free_slots = if cap == CAPACITY_UNLIMITED { None } else { Some(cap) };
            PeerCapabilities {
                proto_version: thrum_core::THRUM_VERSION.to_string(),
                // Match humd's default perch_tag (config::HumConfig::default
                // → nest.default = "claude-repl"). Overflow lookup keys on
                // this nest name; mismatch with the daemon's tag breaks
                // the test deterministically.
                nests: vec!["claude-repl".to_string()],
                free_slots,
                ..Default::default()
            }
        };
        let a_caps = caps_for(&ha);
        let b_caps = caps_for(&hb);
        let (a_view, b_view) = InMemoryEndpoint::pair(
            ha.id, b_caps.clone(),
            hb.id, a_caps.clone(),
        );
        ha.ensemble.install(a_view, a_caps, &a_key);
        hb.ensemble.install(b_view, b_caps, &b_key);
        Ok(())
    }

    /// Federation negative-path test fixture: install A's side honestly
    /// under strict auth, then have C send a **tampered** hello whose
    /// `pubkey` does NOT hash to the claimed `humd_id`. A's drainer
    /// classifies the hello as `Invalid` and ejects the peer entry — so
    /// A's `peers()` won't include C. Both humds must come from
    /// [`Sim::spawn_humd_with_identity`].
    pub fn wire_signed_tampered(&self, a: HumdId, c: HumdId) -> Result<()> {
        let humds = self.humds.read();
        let ha = humds.get(&a).cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", a.short()))?;
        let hc = humds.get(&c).cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", c.short()))?;
        drop(humds);
        let a_key = ha.key.clone().ok_or_else(|| anyhow::anyhow!(
            "humd {} has no signing key", a.short()
        ))?;

        let caps_for = |humd: &SimHumd| {
            let cap = humd.capacity.load(Ordering::SeqCst);
            let free_slots = if cap == CAPACITY_UNLIMITED { None } else { Some(cap) };
            PeerCapabilities {
                proto_version: thrum_core::THRUM_VERSION.to_string(),
                // Match humd's default perch_tag (config::HumConfig::default
                // → nest.default = "claude-repl"). Overflow lookup keys on
                // this nest name; mismatch with the daemon's tag breaks
                // the test deterministically.
                nests: vec!["claude-repl".to_string()],
                free_slots,
                ..Default::default()
            }
        };
        let a_caps = caps_for(&ha);
        let c_caps = caps_for(&hc);
        let (a_view, c_view) = InMemoryEndpoint::pair(
            ha.id, c_caps.clone(),
            hc.id, a_caps.clone(),
        );

        // A installs honestly with strict auth.
        ha.ensemble.install(a_view, a_caps, &a_key);

        // C sends a hello that LIES: the `humd_id` field claims c.id,
        // but the `pubkey` is from a freshly-minted attacker key. The
        // signature is valid for the attacker key over c.id, but
        // `sha256(attacker_pubkey) != c.id`, so A's `parse_hello`
        // returns `Invalid` → A's drainer ejects.
        let attacker_key = HumdKey::generate();
        let tampered = hello_tone(&hc.id, &attacker_key, &c_caps);
        let c_for_send = c_view.clone();
        tokio::spawn(async move {
            let _ = c_for_send.send(tampered).await;
        });
        // C also installs its side (so its drainer exists), but the
        // test only asserts A's view of the registry.
        hc.ensemble.install_unsigned(c_view, c_caps);
        Ok(())
    }

    /// Drop the wired link between `a` and `b`. Both endpoints stop
    /// delivering outbound tones and instead buffer them up to
    /// `ensemble::PARTITION_BUFFER_CAP`. A subsequent [`Sim::heal`]
    /// flushes the buffer to the peer in original order. Errors if
    /// the pair was never wired.
    pub fn partition(&self, a: HumdId, b: HumdId) -> Result<()> {
        let links = self.links.read();
        let link = links
            .get(&link_key(a, b))
            .ok_or_else(|| anyhow::anyhow!("no link {}-{}", a.short(), b.short()))?;
        link.a_end.set_partitioned(true);
        link.b_end.set_partitioned(true);
        Ok(())
    }

    /// Restore the wired link. Flushes any buffered tones in both
    /// directions and then emits a `chi:"wane-sync"` from each side
    /// carrying the local `WaneTracker` snapshot — the receiver merges
    /// by max so wane values reconverge after the partition. v0
    /// reconciliation: just the Lamport tip exchange, no event-log
    /// replay. Errors if the pair was never wired.
    pub async fn heal(&self, a: HumdId, b: HumdId) -> Result<()> {
        let (link_a, link_b) = {
            let links = self.links.read();
            let link = links
                .get(&link_key(a, b))
                .ok_or_else(|| anyhow::anyhow!("no link {}-{}", a.short(), b.short()))?;
            (link.a, link.b)
        };
        {
            let links = self.links.read();
            let link = links.get(&link_key(a, b)).unwrap();
            link.a_end.set_partitioned(false);
            link.b_end.set_partitioned(false);
        }

        let humds = self.humds.read();
        let ha = humds
            .get(&link_a)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", link_a.short()))?;
        let hb = humds
            .get(&link_b)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", link_b.short()))?;
        drop(humds);

        for (from, to) in [(&ha, &hb), (&hb, &ha)] {
            let snapshot = from.waneman.snapshot();
            let mut snapshot_json = serde_json::Map::new();
            for (sigil, n) in snapshot {
                snapshot_json.insert(sigil, Value::from(n));
            }
            let tone = serde_json::json!({
                "chi": "wane-sync",
                "rid": format!("wane-sync-{}", uuid::Uuid::new_v4()),
                "from": from.id.to_hex(),
                "to": to.id.to_hex(),
                "snapshot": Value::Object(snapshot_json),
            });
            if let Err(e) = from.ensemble.route(tone).await {
                tracing::warn!(err = %e, "wane-sync.route.failed");
            }
        }
        Ok(())
    }

    /// Attach a synthetic mock worker bee to `humd`. Registers a fresh
    /// thrum client, hello's it as `bee:["worker"]` advertising `models`,
    /// then spawns a task that turns every inbound chi:"prompt" into
    /// a canned chunk sequence (text_delta "HELLO" + finish/end_turn).
    /// Mirrors what the old in-process `nest::MockWorkerBee` did, just
    /// over the wire so it works under the external-worker model.
    pub async fn attach_mock_worker(&self, humd: HumdId, models: Vec<String>) -> Result<String> {
        let h = self
            .humds
            .read()
            .get(&humd)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", humd.short()))?;
        let client_id = format!("sim-worker-{}", uuid::Uuid::new_v4());
        let mut rx = h.thrum.register_synthetic(client_id.clone());
        // Hello first so humd records bee:["worker"] + models before the
        // first prompt arrives.
        // Use the default hive_tag as the hive name so the ensemble's
        // overflow gossip (which keys peer capabilities by nest name)
        // sees a matching advertised hive. Tests that exercise overflow
        // routing rely on this match.
        let hello = serde_json::json!({
            "chi": "hello",
            "bee": ["worker"],
            "hive": "claude-repl",
            "version": "0.0.0",
            "protoVersion": thrum_core::THRUM_VERSION,
            "models": models,
            "chis": ["hello", "prompt", "chunk", "finish"],
        });
        let thrum_for_hello = h.thrum.clone();
        let cid_for_hello = client_id.clone();
        tokio::spawn(async move {
            thrum_for_hello.inject_tone(&cid_for_hello, hello).await;
        });
        // Pump: on inbound prompt, inject synthetic chunks + finish
        // back through the sink so humd's passthrough block forwards
        // them to the originating nestler's sigil claim.
        let thrum = h.thrum.clone();
        let cid_for_pump = client_id.clone();
        tokio::spawn(async move {
            while let Some(tone) = rx.recv().await {
                let chi = tone.get("chi").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if chi != "prompt" { continue; }
                let sid = tone.get("sid").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if sid.is_empty() { continue; }
                let frames = vec![
                    serde_json::json!({"chi":"chunk","sid":&sid,"chunkType":"text_start","id":0}),
                    serde_json::json!({"chi":"chunk","sid":&sid,"chunkType":"text_delta","delta":"HELLO"}),
                    serde_json::json!({"chi":"chunk","sid":&sid,"chunkType":"content_block_stop","blockIdx":0}),
                    serde_json::json!({"chi":"finish","sid":&sid,"finishReason":"end_turn","usage":{}}),
                ];
                for f in frames {
                    thrum.inject_tone(&cid_for_pump, f).await;
                }
            }
        });
        Ok(client_id)
    }

    /// Mock-send a tone into humd's Thrum as if from a connected
    /// nestler. Registers a synthetic client on first call per humd,
    /// spawns a fanout task that immediately routes incoming tones
    /// into per-sid mailboxes, then injects the tone through the sink.
    /// Returns the synthetic client_id so callers can correlate
    /// replies.
    pub fn nestler_send(&self, humd: HumdId, tone: Value) -> Result<String> {
        let h = self
            .humds
            .read()
            .get(&humd)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", humd.short()))?;
        let client_id = format!("sim-{}", uuid::Uuid::new_v4());
        let mut rx = h.thrum.register_synthetic(client_id.clone());

        // Fanout task: drain this synthetic's outbound queue and route
        // each tone into its sid's mailbox (lazily creating mailboxes
        // for new sids). Without this, tones sit in the mpsc receiver
        // forever and nestler_recv only drains on entry — racing the
        // detached inject_tone.
        let h_for_pump = h.clone();
        tokio::spawn(async move {
            while let Some(tone) = rx.recv().await {
                let sid = tone
                    .get("sid")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if sid.is_empty() { continue; }
                let tx = {
                    let mut senders = h_for_pump.sid_senders.lock();
                    if let Some(tx) = senders.get(&sid) {
                        tx.clone()
                    } else {
                        let (tx, rx2) = mpsc::unbounded_channel::<Value>();
                        senders.insert(sid.clone(), tx.clone());
                        h_for_pump.sid_mailboxes.lock().insert(sid.clone(), rx2);
                        tx
                    }
                };
                let _ = tx.send(tone);
            }
        });

        // Inject on a detached task so the sync API stays sync. The
        // sink is async; we don't want to block the caller while a
        // nest spawns.
        let thrum = h.thrum.clone();
        let cid = client_id.clone();
        tokio::spawn(async move {
            thrum.inject_tone(&cid, tone).await;
        });
        Ok(client_id)
    }

    /// Take the next tone broadcast on `humd` whose `sid` matches.
    /// Best-effort: drains every synthetic out_queue into per-sid
    /// mailboxes, then waits up to `timeout` for the named sid.
    pub async fn nestler_recv(
        &self,
        humd: HumdId,
        sid: &str,
        timeout: Duration,
    ) -> Option<Value> {
        let h = self.humds.read().get(&humd).cloned()?;

        // Bind a mailbox sender for this sid if absent. We pump every
        // synthetic out_queue's items in here, keyed by their `sid`
        // field. Tones without a sid get dropped.
        {
            let mut senders = h.sid_senders.lock();
            let mut mailboxes = h.sid_mailboxes.lock();
            if !senders.contains_key(sid) {
                let (tx, rx) = mpsc::unbounded_channel::<Value>();
                senders.insert(sid.to_string(), tx);
                mailboxes.insert(sid.to_string(), rx);
            }
        }

        // Drain all out_queues into the per-sid mailboxes. We move
        // each receiver out, drain non-blockingly, then put it back.
        // This keeps the per-humd state simple — no long-lived fanout
        // task per synthetic client.
        let mut queues = h.out_queues.lock();
        for (_cid, rx) in queues.iter_mut() {
            while let Ok(tone) = rx.try_recv() {
                let tone_sid = tone
                    .get("sid")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if tone_sid.is_empty() {
                    continue;
                }
                let senders = h.sid_senders.lock();
                if let Some(tx) = senders.get(&tone_sid) {
                    let _ = tx.send(tone);
                } else {
                    drop(senders);
                    // Lazily mint a mailbox so future recv calls for
                    // this sid see the message.
                    let (tx, rx2) = mpsc::unbounded_channel::<Value>();
                    let _ = tx.send(tone);
                    h.sid_senders.lock().insert(tone_sid.clone(), tx);
                    h.sid_mailboxes.lock().insert(tone_sid, rx2);
                }
            }
        }
        drop(queues);

        // Now await the named sid's mailbox.
        let mut rx_opt = h.sid_mailboxes.lock().remove(sid)?;
        let result = tokio::time::timeout(timeout, rx_opt.recv()).await.ok().flatten();
        // Put the mailbox back so subsequent recvs can use it.
        h.sid_mailboxes.lock().insert(sid.to_string(), rx_opt);
        result
    }

    /// Mock-attach a hearOnly observer nestler on `observer_humd` to
    /// the hum `sid` hosted on `host_humd`. Returns the synthetic client
    /// id so the caller can drain replies via `nestler_recv`. Sends a
    /// `chi:"attach"` tone whose `to:` is the host and whose `from:` is
    /// the observer — the standard cross-humd routing path delivers it.
    pub fn attach_observer(
        &self,
        observer_humd: HumdId,
        host_humd: HumdId,
        sid: &str,
    ) -> Result<String> {
        self.nestler_send(
            observer_humd,
            serde_json::json!({
                "chi": "attach",
                "rid": format!("attach-{}", uuid::Uuid::new_v4()),
                "sid": sid,
                "to": host_humd.to_hex(),
                "from": observer_humd.to_hex(),
                "hearOnly": true,
            }),
        )
    }

    /// Tap the next tone arriving from a peer (via the ensemble) at
    /// `humd`. Returns `None` on timeout. Useful for tests that prove
    /// pure routing — no nest, no sid claim, no broadcast required.
    pub async fn humd_peer_tap(&self, humd: HumdId, timeout: Duration) -> Option<Value> {
        let h = self.humds.read().get(&humd).cloned()?;
        let mut rx = h.ensemble.subscribe();
        match tokio::time::timeout(timeout, rx.recv()).await {
            Ok(Ok(tone)) => Some(tone),
            _ => None,
        }
    }

    /// Shutdown all humds and drain their join handles.
    pub async fn shutdown(self) {
        let humds: Vec<Arc<SimHumd>> = self.humds.read().values().cloned().collect();
        for h in &humds {
            if let Some(tx) = h.shutdown_tx.lock().take() {
                let _ = tx.send(());
            }
        }
        for h in &humds {
            let join = h.join.lock().take();
            if let Some(j) = join {
                let _ = j.await;
            }
        }
    }
}

// Silence the "field never read" warning for penny_path placeholder.
#[allow(dead_code)]
fn _keep_path_alive(_p: PathBuf) {}

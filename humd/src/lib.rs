//! humd — the hum daemon as a library.
//!
//! `cargo run -p humd` is the binary at `src/main.rs`; this lib is the
//! reusable boot path. Tests, simulators (`p2p-sim`), and embedders all
//! call [`run`] with a [`DaemonConfig`] instead of duplicating the wiring.
//!
//! Boot order is fixed: state crates → trackers → nest pool → thrum →
//! MCP → wait for shutdown. Tracing setup is the binary's job; the lib
//! never touches the global subscriber.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use ensemble::{Ensemble, HumdAddr, HumdId, HumdKey, PeerCapabilities};
use mcp::{serve as mcp_serve, Registry as McpRegistry};
use parking_lot::RwLock;
use serde_json::Value;
use thrumd::{serve as thrum_serve, Thrum, Tone, ToneSink};
use thrum_core::{Chi, WaneTracker};
use tracing::{info, trace, warn};

mod identity;
mod peers;
pub use identity::{key_path, load_or_mint_key};
pub use peers::{peers_path, PeerConfig};

/// Per-sid observer roster. Maps a hum's `sid` to a list of peer humds
/// that have asked (via `chi:"attach"`) to receive a copy of every
/// outbound reply tone. Shared between the HumdSink (writer on attach /
/// detach) and every NestListener serving that sid (reader on each
/// reply).
type Observers = Arc<RwLock<HashMap<String, Vec<HumdId>>>>;

// ── Public config ──────────────────────────────────────────────────────────

/// Where the daemon listens, and how it should pace itself. Construct
/// via [`DaemonConfig::from_env`] for production defaults or build one
/// by hand for tests / simulators.
///
/// humd is a router: worker bees register over thrum via `chi:"hello"`
/// with `bee: ["worker"]`. There's no in-process worker hosting anymore.
/// The sim hooks layer on top: `thrum_override` (caller-owned Thrum,
/// skip the socket listener), `ensemble` (peer registry for `to:`-
/// addressed tones), `bind_mcp` (skip the HTTP MCP listener).
pub struct DaemonConfig {
    pub thrum_path: PathBuf,
    pub http_path: PathBuf,
    pub mcp_addr: std::net::SocketAddr,
    pub penny_path: PathBuf,
    pub hum_cfg: config::HumConfig,
    pub cli_path: String,
    pub penny_persist_interval: Duration,
    /// When set, sim provides the Thrum and the daemon does NOT bind a
    /// unix socket. When None, humd builds its own Thrum and binds.
    pub thrum_override: Option<Thrum>,
    /// When set, daemon installs this Ensemble for inter-humd routing.
    /// When None, the daemon runs without a peer set (legacy single-host).
    pub ensemble: Option<Arc<Ensemble>>,
    /// When false, skip mcp_serve too (sim doesn't need a live HTTP MCP).
    pub bind_mcp: bool,
    /// Cap on concurrent local hums. `Some(0)` means "always overflow to a
    /// peer"; `None` means unbounded (legacy behaviour). Used by the
    /// overflow-routing policy in the prompt arm of [`HumdSink::hear`].
    pub capacity_override: Option<usize>,
    /// Caller-owned WaneTracker. Sim supplies one so it can read/write
    /// wane values from the test driver. Production leaves this None and
    /// the daemon mints its own. Either way the sink uses the same
    /// shared Arc.
    pub waneman: Option<Arc<WaneTracker>>,
    /// Persistent humd identity. `from_env` loads (or mints + persists)
    /// from `$XDG_STATE_HOME/hum/humd.key`. Tests / sims leave this None
    /// and continue to generate ephemeral keys per spawn.
    pub humd_key: Option<Arc<HumdKey>>,
    /// Peers to dial on boot. `from_env` reads
    /// `$XDG_CONFIG_HOME/hum/peers.json`; missing file = empty list.
    pub bootstrap_peers: Vec<PeerConfig>,
}

impl DaemonConfig {
    pub fn from_env() -> Self {
        let runtime_dir = runtime_dir();
        // Canonical thrum socket path — honors HUM_THRUM_SOCK (and the
        // legacy HUM_SOCKET fallback). thrumd owns the source of truth;
        // humd just reuses it so binary + protocol agree.
        let thrum_path = thrumd::default_socket_path();
        let http_path = runtime_dir.join("hum.sock.http");
        let mcp_port: u16 = std::env::var("HUM_MCP_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(29147);
        // Identity is not fatal — if the state dir is unwritable we log
        // and run without a persisted key (legacy single-host).
        let humd_key = match identity::load_or_mint_key() {
            Ok(k) => Some(Arc::new(k)),
            Err(e) => {
                warn!(err = %e, "identity.load.failed");
                None
            }
        };
        let bootstrap_peers = peers::load();
        Self {
            thrum_path,
            http_path,
            mcp_addr: ([127, 0, 0, 1], mcp_port).into(),
            penny_path: runtime_dir.join("penny.json"),
            hum_cfg: config::load(),
            cli_path: std::env::var("CLAUDE_CLI_PATH").unwrap_or_else(|_| "claude".into()),
            penny_persist_interval: Duration::from_secs(10),
            thrum_override: None,
            ensemble: None,
            bind_mcp: true,
            capacity_override: None,
            waneman: None,
            humd_key,
            bootstrap_peers,
        }
    }
}

fn runtime_dir() -> PathBuf {
    let base = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"));
    base.join("hum")
}


// ── Public entry point ─────────────────────────────────────────────────────

/// Build a daemon and run until `shutdown` resolves.
///
/// `shutdown` is parameterized so the binary can plug ctrl-c/SIGTERM
/// while the simulator can plug an in-memory cancel token. The function
/// returns once the shutdown future resolves AND best-effort state has
/// been flushed.
pub async fn run<F>(mut cfg: DaemonConfig, shutdown: F) -> Result<()>
where
    F: std::future::Future<Output = ()> + Send,
{
    info!(
        thrum = %cfg.thrum_path.display(),
        http = %cfg.http_path.display(),
        mcp = %cfg.mcp_addr,
        "humd.sockets"
    );

    let _hums = hums::Hums::load();
    let penny = penny::Penny::load(&cfg.penny_path);
    penny.clone().spawn_persister(cfg.penny_path.clone(), cfg.penny_persist_interval);

    let waneman = cfg.waneman.clone().unwrap_or_else(|| Arc::new(WaneTracker::new()));
    let _drift = drift::Drift::new();
    let _drone = drone::Drone::new();

    // Bring up an Ensemble from the persisted identity when the caller
    // didn't supply one. Sim provides its own pre-wired Ensemble (with
    // InMemoryEndpoints); the production binary lets us mint one here so
    // peer dialling has something to install into.
    let ensemble_opt: Option<Arc<Ensemble>> = match cfg.ensemble.clone() {
        Some(e) => Some(e),
        None => cfg.humd_key.as_ref().map(|k| {
            let me = k.humd_id();
            info!(humd_id = %me, "ensemble.boot");
            Arc::new(Ensemble::new(me))
        }),
    };

    // Dial-on-boot. Each peer attempt is independently fallible — one
    // dead host shouldn't sink startup. TcpTransport is the only wired
    // transport today; other hint prefixes ("iroh:") are skipped with a
    // log line until their transports land.
    if let (Some(ens), Some(_key)) = (&ensemble_opt, &cfg.humd_key) {
        let my_caps = my_capabilities(&cfg);
        for peer in &cfg.bootstrap_peers {
            let tcp_hint = peer
                .hints
                .iter()
                .find_map(|h| h.strip_prefix("tcp:"));
            let Some(addr) = tcp_hint else {
                trace!(peer = %peer.humd_id.short(), "peers.skip.no-tcp-hint");
                continue;
            };
            let peer_addr = {
                let mut a = HumdAddr::new(peer.humd_id);
                for h in &peer.hints { a.hints.push(h.clone()); }
                a
            };
            match ensemble::TcpEndpoint::connect(addr, peer_addr, PeerCapabilities::default()).await {
                Ok(conn) => {
                    info!(peer = %peer.humd_id.short(), addr, "peer.dial.ok");
                    ens.add_peer_with_caps(conn as Arc<dyn ensemble::PeerConnection>, my_caps.clone());
                }
                Err(e) => {
                    warn!(peer = %peer.humd_id.short(), addr, err = %e, "peer.dial.failed");
                }
            }
        }
    } else if !cfg.bootstrap_peers.is_empty() {
        warn!(
            count = cfg.bootstrap_peers.len(),
            "peers.skip.no-identity-or-ensemble"
        );
    }

    // Stash so the rest of run() keeps working off the cfg-or-minted
    // ensemble instead of just cfg.ensemble.
    let ensemble_for_sink = ensemble_opt.clone();

    // humd no longer hosts an in-process nest pool — worker bees
    // register over thrum as separate processes. Configuration
    // (max_procs, idle_threshold) becomes the worker's concern.
    let mcp_url = format!("http://{}", cfg.mcp_addr);

    // Caller-owned Thrum (sim) vs daemon-owned (production). When the
    // caller owns it, we install our sink onto theirs and never bind.
    let is_embedded = cfg.thrum_override.is_some();
    let (thrum, bind_thrum) = match cfg.thrum_override.take() {
        Some(t) => (t, false),
        None => (Thrum::new(), true),
    };
    let observers: Observers = Arc::new(RwLock::new(HashMap::new()));
    // MCP registry has to outlive its serve task because HumdSink also
    // pokes it (sets per-session nestler_tools when chi:"prompt" carries
    // body.tools). Build it once, share via clone.
    let mcp_registry = McpRegistry::new();
    // Pending nestler-tool dispatches keyed by callId. The NestlerHook
    // installs a oneshot Sender on each call; the chi:"tool-result"
    // handler resolves it. Lives on the sink so both halves can reach it.
    let tool_pending: ToolPending = Arc::new(parking_lot::Mutex::new(HashMap::new()));
    // Install the bridge: any nestler-tool MCP call humd's MCP server
    // dispatches gets translated into a chi:"tool-call" tone routed to
    // the originating client. The thrum sigil claim made at chi:"prompt"
    // time guarantees the tone lands on the right consumer.
    let hive_tag = cfg.hum_cfg.nest.default.clone();
    mcp_registry.set_nestler_hook(Arc::new(NestlerBridge {
        thrum: thrum.clone(),
        pending: tool_pending.clone(),
        hive_tag: hive_tag.clone(),
    }));
    // Native MCP tool completions become chi:"tool-info" events on
    // the wire. Observers see {sid, name, args, result, source} as a
    // single semantic event — no need to reconstitute from chunks.
    mcp_registry.set_tool_info_hook(Arc::new(ToolInfoBroadcaster {
        thrum: thrum.clone(),
        hive_tag: hive_tag.clone(),
    }));
    let manifests: Manifests = Arc::new(parking_lot::RwLock::new(HashMap::new()));
    let sid_origins: Arc<parking_lot::RwLock<HashMap<String, ensemble::HumdId>>> =
        Arc::new(parking_lot::RwLock::new(HashMap::new()));
    let sink: Arc<dyn ToneSink> = Arc::new(HumdSink {
        thrum: thrum.clone(),
        waneman: waneman.clone(),
        mcp_url: mcp_url.clone(),
        cli_path: cfg.cli_path.clone(),
        ensemble: ensemble_for_sink.clone(),
        observers: observers.clone(),
        capacity_override: cfg.capacity_override,
        mcp_registry: mcp_registry.clone(),
        tool_pending: tool_pending.clone(),
        hive_tag: hive_tag.clone(),
        manifests: manifests.clone(),
        sid_origins: sid_origins.clone(),
    });
    thrum.set_sink(sink);
    if bind_thrum {
        let thrum = thrum.clone();
        let path = cfg.thrum_path.clone();
        tokio::spawn(async move {
            if let Err(e) = thrum_serve(thrum, &path).await {
                warn!(err = %e, "thrum.exit");
            }
        });
    } else {
        trace!("thrum.override.installed");
    }

    if cfg.bind_mcp {
        let mcp_addr = cfg.mcp_addr;
        let registry = mcp_registry.clone();
        tokio::spawn(async move {
            if let Err(e) = mcp_serve(mcp_addr, registry).await {
                warn!(err = %e, "mcp.exit");
            }
        });
    } else {
        trace!("mcp.bind.skipped");
    }

    // Ensemble inbound pump — every tone arriving from a peer humd is
    // injected back through our own Thrum's sink as if it had arrived
    // from a special "ensemble" client. This is how `to:`-routed tones
    // from peer-A reach peer-B's HumdSink dispatch.
    if let Some(ens) = ensemble_for_sink.clone() {
        let mut rx = ens.subscribe();
        let thrum_for_pump = thrum.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(tone) => {
                        thrum_for_pump.inject_tone("ensemble", tone).await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(skipped = n, "ensemble.inbox.lagged");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            trace!("ensemble.inbox.closed");
        });
    }

    // Auto-update — in-process daily check. Skipped under sim/test
    // (caller-owned thrum means we're embedded in someone else's
    // runtime, not a real boot). The job shells to curl + the
    // canonical installer; the installer rebuilds humd and bounces the
    // service, which kills this task naturally.
    if !is_embedded {
        tokio::spawn(autoupdate_loop());
    }

    info!("humd.ready");
    shutdown.await;
    info!("humd.shutting-down");
    if let Err(e) = penny.save(&cfg.penny_path) {
        warn!(err = %e, "penny.save.failed");
    }
    info!("humd.exit");
    Ok(())
}

/// Daily self-update — once every 24h, compare the running version to
/// the upstream release and re-run the canonical installer if newer.
///
/// Single retry on transient network failure (15-min retry, then back
/// to the daily cadence). Errors are logged but never fatal — a humd
/// that can't reach github should keep humming.
async fn autoupdate_loop() {
    // Initial sleep — avoid update storm at boot if the user just
    // ran `./install` (which already pulled the latest).
    tokio::time::sleep(Duration::from_secs(6 * 60 * 60)).await;
    let mut interval = tokio::time::interval(Duration::from_secs(24 * 60 * 60));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        match autoupdate_check_once().await {
            Ok(true) => info!("autoupdate.applied"),
            Ok(false) => trace!("autoupdate.up-to-date"),
            Err(e) => {
                warn!(err = %e, "autoupdate.failed");
                tokio::time::sleep(Duration::from_secs(15 * 60)).await;
            }
        }
    }
}

/// One iteration of the auto-update check. Returns Ok(true) if the
/// installer ran, Ok(false) if we're already up to date.
async fn autoupdate_check_once() -> Result<bool> {
    let local = env!("CARGO_PKG_VERSION").to_string();
    let body = tokio::process::Command::new("curl")
        .args([
            "-fsSL",
            "-H", "Accept: application/vnd.github+json",
            "https://api.github.com/repos/adiled/hum/releases/latest",
        ])
        .output()
        .await?;
    if !body.status.success() {
        anyhow::bail!("github releases fetch failed: {}", body.status);
    }
    let body = String::from_utf8(body.stdout)?;
    // Inline `"tag_name":"vX.Y.Z"` lookup — avoids dragging a JSON
    // parser into this hot path for one field.
    let upstream = parse_tag_name(&body).ok_or_else(|| anyhow::anyhow!("no tag_name in response"))?;
    let upstream_trim = upstream.trim_start_matches('v');
    if upstream_trim == local {
        return Ok(false);
    }
    info!(local = %local, upstream = %upstream_trim, "autoupdate.newer.found");
    let status = tokio::process::Command::new("bash")
        .arg("-c")
        .arg("curl -fsSL https://raw.githubusercontent.com/adiled/hum/main/install | bash")
        .status()
        .await?;
    if !status.success() {
        anyhow::bail!("installer exited with {status}");
    }
    Ok(true)
}

fn parse_tag_name(body: &str) -> Option<String> {
    let needle = "\"tag_name\":";
    let start = body.find(needle)? + needle.len();
    let rest = &body[start..];
    let q1 = rest.find('"')? + 1;
    let q2 = rest[q1..].find('"')?;
    Some(rest[q1..q1 + q2].to_string())
}

// ── ToneSink — the big chi dispatch ────────────────────────────────────────

struct HumdSink {
    thrum: Thrum,
    waneman: Arc<WaneTracker>,
    mcp_url: String,
    cli_path: String,
    /// When present, tones with a `to:` hex addressed to a *different*
    /// humd are routed through here instead of being dispatched locally.
    ensemble: Option<Arc<Ensemble>>,
    /// Per-sid roster of peer humds tapping us in `hearOnly` mode. Read
    /// by every NestListener on each reply; written here on `attach` /
    /// `detach`.
    observers: Observers,
    /// Concurrent-hum cap. `Some(0)` triggers overflow routing in the
    /// prompt arm — local prompts get forwarded to a peer with spare
    /// capacity instead of awakening here. `None` = unbounded.
    capacity_override: Option<usize>,
    /// MCP registry — populated per-session with nestler-declared tools
    /// extracted from chi:"prompt".tools so the worker bee's MCP client
    /// sees them advertised alongside humd's native tools.
    mcp_registry: McpRegistry,
    /// Pending dispatches of nestler-declared tool calls, keyed by the
    /// callId we minted when issuing chi:"tool-call". Resolved when the
    /// matching chi:"tool-result" lands.
    tool_pending: ToolPending,
    /// Routing tag used in thrum_broadcast + sigil for all reply tones.
    /// Comes from cfg.hum_cfg.nest.default so the daemon never hardcodes
    /// a specific hive name; whichever hive is configured is the tag.
    hive_tag: String,
    /// Live registry of every chi:"hello" we've received. Keyed by
    /// thrum client_id. Routing scans this for bee/models matches.
    manifests: Manifests,
    /// Map sid → originating peer humd. Populated in the prompt arm
    /// when a prompt arrives via the ensemble pump (client_id ==
    /// "ensemble" and the tone carries a peer `from` humd id). Read
    /// in the passthrough block: chi:"chunk"/"finish" tones whose
    /// sid is in this map get stamped `to: <origin>` and routed via
    /// the ensemble back to the originating humd.
    sid_origins: Arc<parking_lot::RwLock<HashMap<String, ensemble::HumdId>>>,
}

/// Map of in-flight nestler-tool call ids → oneshot senders waiting
/// for the result content. Shared between the NestlerBridge (writer
/// on dispatch) and the chi:"tool-result" handler (resolver).
type ToolPending = Arc<parking_lot::Mutex<HashMap<String, tokio::sync::oneshot::Sender<String>>>>;

/// Live registry of every client that handshook via `chi:"hello"`.
/// Keyed by thrum client_id. The manifest carries `bee` kinds,
/// advertised models, propensity, chi vocabulary, etc. — humd queries
/// this to route prompts to worker bees, to identify which clients are
/// workers vs foragers, and to fan out gossip on connect.
///
/// Volatile: cleared on humd restart; entries pruned on disconnect.
/// Same data shape ensemble gossips, just held in-process for
/// routing decisions humd makes locally.
type Manifests = Arc<parking_lot::RwLock<HashMap<String, ensemble::HiveManifest>>>;

/// MCP NestlerHook impl that round-trips nestler-declared tool calls
/// back to the originator over thrum. The worker bee's MCP client
/// (whatever worker is running) sees these tools advertised alongside
/// humd's native ones; when the model invokes one, the call lands here.
/// Hum-native MCP tools resolve in-process without leaving humd.
///
/// 1. worker calls tool with args → MCP server dispatches → us.
/// 2. We mint a callId, store a oneshot tx in `pending`, and emit
///    `chi:"tool-call" {sid, callId, name, args}`. The thrum sigil
///    claim placed at chi:"prompt" time routes the tone to the
///    originating client.
/// 3. Originator executes externally, replies with
///    `chi:"tool-result" {sid, callId, result}`. humd's
///    chi:"tool-result" arm pops the sender and forwards the result.
/// 4. dispatch() returns; MCP packages the result; worker continues.
struct NestlerBridge {
    thrum: Thrum,
    pending: ToolPending,
    /// Hive name used as the broadcast tag for the chi:"tool-call"
    /// tone. Comes from cfg.hum_cfg.nest.default so the routing tag
    /// matches whatever hive humd is running, not a hardcoded name.
    hive_tag: String,
}

#[async_trait::async_trait]
impl mcp::NestlerHook for NestlerBridge {
    async fn dispatch(
        &self,
        session_id: &str,
        tool: &str,
        args: Value,
    ) -> anyhow::Result<String> {
        let call_id = format!("call-{}", thrum_core::rid());
        let (tx, rx) = tokio::sync::oneshot::channel::<String>();
        self.pending.lock().insert(call_id.clone(), tx);
        let tone = serde_json::json!({
            "chi": "tool-call",
            "sid": session_id,
            "callId": call_id,
            "name": tool,
            "args": args,
        });
        // sigil claim made at chi:"prompt" time gets this back to the
        // originator. Broadcast tag is the configured hive name.
        self.thrum.thrum_broadcast(session_id, &self.hive_tag, tone);
        match tokio::time::timeout(Duration::from_secs(300), rx).await {
            Ok(Ok(result)) => {
                // Emit a chi:"tool-info" tone with the completed pair
                // so observers (drone, dashboards, rich bees) can
                // render the full call+result without subscribing to
                // chunk-level fragments.
                let info = serde_json::json!({
                    "chi": "tool-info",
                    "sid": session_id,
                    "callId": call_id,
                    "name": tool,
                    "args": args,
                    "result": result,
                    "source": "nestler",
                });
                self.thrum.thrum_broadcast(session_id, &self.hive_tag, info);
                Ok(result)
            }
            Ok(Err(_)) => {
                self.pending.lock().remove(&call_id);
                anyhow::bail!("nestler tool-result channel closed")
            }
            Err(_) => {
                self.pending.lock().remove(&call_id);
                anyhow::bail!("nestler tool-result timed out (300s)")
            }
        }
    }
}

/// MCP ToolInfoHook impl — turns native MCP tool completions into
/// chi:"tool-info" tones on thrum. Pure observation; doesn't affect
/// MCP dispatch in any way.
struct ToolInfoBroadcaster {
    thrum: Thrum,
    hive_tag: String,
}

impl mcp::ToolInfoHook for ToolInfoBroadcaster {
    fn record(
        &self,
        session_id: &str,
        tool: &str,
        args: Value,
        result: &str,
        source: mcp::ToolInfoSource,
    ) {
        let source_tag = match source {
            mcp::ToolInfoSource::Native => "native",
            mcp::ToolInfoSource::External => "external",
        };
        let tone = serde_json::json!({
            "chi": "tool-info",
            "sid": session_id,
            "name": tool,
            "args": args,
            "result": result,
            "source": source_tag,
        });
        self.thrum.thrum_broadcast(session_id, &self.hive_tag, tone);
    }
}

// (NestListener removed — worker bees emit chi:"chunk"/"finish"
// directly over thrum from their own process. humd just routes; see
// the passthrough block in ToneSink::hear that re-broadcasts those
// tones on the sigil sid claimed by the originating forager bee.)

#[async_trait::async_trait]
impl ToneSink for HumdSink {
    async fn hear(&self, client_id: &str, tone: Tone) {
        let chi_str = tone.get("chi").and_then(Value::as_str).unwrap_or("?");
        let chi: Option<Chi> = tone
            .get("chi")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok());

        // Worker passthrough: any output tone (chunk / finish / error /
        // tool-call / tool-info / session-ready) coming from a client
        // whose manifest declares `bee` containing "worker" gets
        // re-broadcast on the sid sigil so the originating forager
        // receives it. Hello arrives from the worker but is handled
        // normally below.
        if !matches!(chi, Some(Chi::Hello)) {
            let is_worker = {
                let m = self.manifests.read();
                m.get(client_id)
                    .map(|man| man.bee.iter().any(|b| b == "worker"))
                    .unwrap_or(false)
            };
            if is_worker {
                if let Some(sid) = tone.get("sid").and_then(Value::as_str).map(str::to_string) {
                    if matches!(chi,
                        Some(Chi::Chunk) | Some(Chi::Finish) | Some(Chi::Error)
                        | Some(Chi::ToolCall) | Some(Chi::ToolInfo) | Some(Chi::SessionReady)
                        | Some(Chi::Pulse) | Some(Chi::Breath)
                    ) {
                        // Fan out to every peer humd that attached as
                        // a hearOnly observer on this sid. Each gets
                        // its own copy stamped `to: <observer>` so the
                        // ensemble routes correctly. Await in-line so
                        // chunk ordering is preserved across multi-
                        // chunk turns (spawn would race chunks vs
                        // finish at the receiver).
                        if let Some(ens) = &self.ensemble {
                            let obs = self.observers.read().get(&sid).cloned().unwrap_or_default();
                            for peer in obs {
                                let mut copy = tone.clone();
                                if let Some(obj) = copy.as_object_mut() {
                                    obj.insert("to".into(), Value::String(peer.to_hex()));
                                    obj.insert("from".into(), Value::String(ens.me().to_hex()));
                                }
                                if let Err(e) = ens.route(copy).await {
                                    warn!(err = %e, "worker.reply.observer.failed");
                                }
                            }
                        }
                        // Cross-humd return path: if the prompt arrived
                        // from a peer humd, stamp `to: <origin>` and
                        // route via the ensemble back to it.
                        let origin = self.sid_origins.read().get(&sid).cloned();
                        if let (Some(origin), Some(ens)) = (origin, &self.ensemble) {
                            let mut copy = tone.clone();
                            if let Some(obj) = copy.as_object_mut() {
                                obj.insert("to".into(), Value::String(origin.to_hex()));
                                obj.insert("from".into(), Value::String(ens.me().to_hex()));
                            }
                            if let Err(e) = ens.route(copy).await {
                                warn!(err = %e, "worker.reply.ensemble.failed");
                            }
                        }
                        self.thrum.thrum_broadcast(&sid, &self.hive_tag, tone.clone());
                        return;
                    }
                }
            }
        }

        // Attach from a local nestler addressed at a peer humd needs a
        // sigil claim here *before* the cross-humd router whisks the tone
        // away — otherwise reply tones flowing back across the ensemble
        // pump get broadcast on the sid and find no claimant, falling
        // back to the unregistered-clients branch by luck. Claim first,
        // then let the standard routing block forward.
        if matches!(chi, Some(Chi::Attach)) && client_id != "ensemble" {
            if let Some(sid) = tone.get("sid").and_then(Value::as_str) {
                if !sid.is_empty() {
                    self.thrum.claim_sigil(client_id, thrum_core::sigil(sid, &self.hive_tag));
                    self.thrum.claim_sigil(client_id, sid.to_string());
                    let hear_only = tone.get("hearOnly").and_then(Value::as_bool).unwrap_or(false);
                    trace!(client_id, sid, hear_only, "attach.local.claimed");
                }
            }
        }

        // Cross-humd routing — if tone is addressed to a *different* humd
        // and we have an Ensemble, hand it off and stop here. Without an
        // Ensemble, `to:` is ignored (legacy single-host behaviour).
        if let Some(ensemble) = &self.ensemble {
            if let Some(to) = tone.get("to").and_then(Value::as_str) {
                if !to.is_empty() && to != ensemble.me().to_hex() {
                    trace!(client_id, %chi_str, to, "ensemble.route");
                    if let Err(e) = ensemble.route(tone).await {
                        warn!(client_id, err = %e, "ensemble.route.failed");
                    }
                    return;
                }
            }
        }

        // Inbound from a peer humd carrying daemon→nestler chi: forward
        // to local clients claiming the sid (the synthetic nestler that
        // originated the prompt). Without this, replies routed back
        // across the ensemble hit the daemon but never reach the
        // nestler tap.
        if client_id == "ensemble" {
            let is_reply = matches!(
                chi,
                Some(Chi::Chunk)
                    | Some(Chi::Finish)
                    | Some(Chi::Error)
                    | Some(Chi::SessionReady)
                    | Some(Chi::Pulse)
                    | Some(Chi::ToolCall)
                    | Some(Chi::ToolMeta)
                    | Some(Chi::PermissionAsk)
            );
            if is_reply {
                let sid_opt = tone.get("sid").and_then(Value::as_str).map(str::to_string);
                if let Some(sid) = sid_opt {
                    trace!(client_id, %chi_str, %sid, "thrum.recv.peer-reply.forward");
                    self.thrum.thrum_broadcast(&sid, &self.hive_tag, tone);
                    return;
                }
            }
        }

        match chi {
            Some(Chi::Hello) => {
                trace!(client_id, %chi_str, "thrum.recv.hello");
                let breath = thrumd::breath_tone(serde_json::json!({}));
                self.thrum.thrum_to(client_id, breath);

                // Build the manifest once from the hello tone.
                // Bee + models come from the new fields; everything
                // else (name, version, proto, propensity, chis, source,
                // bind, nestlerId) lives where it always did.
                let bee: Vec<String> = match tone.get("bee") {
                    Some(Value::Array(arr)) => arr.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect(),
                    Some(Value::String(s)) => vec![s.clone()],
                    _ => Vec::new(),
                };
                let models: Vec<String> = tone.get("models")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                    .unwrap_or_default();
                let name = tone.get("hive").and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| tone.get("from").and_then(Value::as_str).map(str::to_string));
                if let Some(name) = name {
                    let proto = tone.get("protoVersion").and_then(Value::as_str)
                        .unwrap_or(thrum_core::THRUM_VERSION).to_string();
                    let version = tone.get("version").and_then(Value::as_str)
                        .unwrap_or("0.0.0").to_string();
                    let propensity = tone.get("propensity")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    let chis: Vec<String> = tone.get("chis")
                        .or_else(|| tone.get("chi"))
                        .and_then(|v| v.as_array())
                        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                        .unwrap_or_default();
                    let source = tone.get("source").and_then(Value::as_str).map(str::to_string);
                    let bind: Option<ensemble::BindAddr> = tone.get("bind")
                        .and_then(|v| serde_json::from_value(v.clone()).ok());
                    let nestler_id = tone.get("nestlerId").and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| {
                            let ms = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis() as u64)
                                .unwrap_or(0);
                            format!("{}-{}", client_id, ms)
                        });
                    let mut manifest = ensemble::HiveManifest::new(name, version, proto);
                    manifest.propensity = propensity;
                    manifest.chis = chis;
                    manifest.source = source;
                    manifest.bind = bind;
                    manifest.nestler_id = Some(nestler_id);
                    manifest.bee = bee.clone();
                    manifest.models = models.clone();

                    // Local store — single source of truth for routing
                    // decisions humd makes (worker lookup, passthrough
                    // membership). Pruned on disconnect.
                    if client_id != "ensemble" {
                        self.manifests.write().insert(client_id.to_string(), manifest.clone());
                        if bee.iter().any(|b| b == "worker") {
                            info!(client_id, ?models, "worker.registered");
                        }
                    }

                    // Gossip to peers via ensemble. Same manifest. Peer
                    // humds learn about workers and foragers the same way.
                    if client_id != "ensemble" {
                        if let Some(ensemble) = &self.ensemble {
                            let ens = ensemble.clone();
                            tokio::spawn(async move {
                                ens.hive_advertise(manifest).await;
                            });
                        }
                    }
                }
            }
            Some(Chi::Prompt) => {
                let sid = tone.get("sid").and_then(Value::as_str).unwrap_or("").to_string();
                if sid.is_empty() {
                    warn!(client_id, "prompt.no-sid");
                    return;
                }
                // Overflow routing — if this prompt came from a local
                // forager AND we have no spare capacity, hand it to a
                // peer that advertises the hive with free slots.
                // Prompts arriving from a peer (`client_id == "ensemble"`)
                // are work *we* accepted from somebody else; never bounce
                // them again.
                if client_id != "ensemble"
                    && self.capacity_override.map(|c| c == 0).unwrap_or(false)
                {
                    if let Some(ensemble) = &self.ensemble {
                        let target = pick_overflow_peer(ensemble, &self.hive_tag);
                        if let Some(peer) = target {
                            // Claim the sid so reply tones (chunks +
                            // finish) routed back via the ensemble pump
                            // reach this client's queue.
                            self.thrum.claim_sigil(client_id, &thrum_core::sigil(&sid, &self.hive_tag));
                            self.thrum.claim_sigil(client_id, &sid);
                            if let Some(rid) = tone.get("rid").and_then(Value::as_str) {
                                self.thrum.thrum_to(client_id, thrumd::echo_tone(rid, true, None));
                            }
                            let mut forward = tone.clone();
                            if let Some(obj) = forward.as_object_mut() {
                                obj.insert("to".into(), Value::String(peer.to_hex()));
                                obj.insert("from".into(), Value::String(ensemble.me().to_hex()));
                            }
                            trace!(sid, peer = %peer.short(), "overflow.route");
                            if let Err(e) = ensemble.route(forward).await {
                                warn!(sid, err = %e, "overflow.route.failed");
                            }
                            return;
                        } else {
                            warn!(sid, "overflow.no-route");
                        }
                    }
                }
                let model = tone.get("modelId").and_then(Value::as_str).unwrap_or("sonnet").to_string();
                let cwd = tone.get("cwd").and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| "/".into()));
                let system_prompt = tone.get("systemPrompt").and_then(Value::as_str).map(str::to_string);
                let text = tone.get("text").and_then(Value::as_str).map(str::to_string)
                    .or_else(|| tone.get("content").and_then(Value::as_str).map(str::to_string))
                    .unwrap_or_default();
                if let Some(rid) = tone.get("rid").and_then(Value::as_str) {
                    self.thrum.thrum_to(client_id, thrumd::echo_tone(rid, true, None));
                }
                self.thrum.claim_sigil(client_id, &thrum_core::sigil(&sid, &self.hive_tag));
                self.thrum.claim_sigil(client_id, &sid);
                // Origin detection: if the prompt arrived from a peer
                // (we received it via the ensemble pump, whose synthetic
                // client_id is "ensemble", and the tone carries a `from`
                // humd id that isn't us), reply tones must route back
                // there via the ensemble.
                let origin = if client_id == "ensemble" {
                    tone.get("from")
                        .and_then(Value::as_str)
                        .and_then(parse_humd_id)
                        .filter(|h| {
                            self.ensemble
                                .as_ref()
                                .map(|e| *h != e.me())
                                .unwrap_or(false)
                        })
                } else {
                    None
                };
                trace!(sid, model, ?origin, "thrum.recv.prompt");

                // Look up a registered worker bee by advertised model —
                // scan manifests for bee.contains("worker") + a matching
                // models entry. Lazy-prune stale entries (worker
                // disconnected since hello) on the same pass.
                let worker_client = {
                    let mut to_prune: Vec<String> = Vec::new();
                    let pick = {
                        let m = self.manifests.read();
                        let mut found: Option<String> = None;
                        for (cid, man) in m.iter() {
                            if man.bee.iter().any(|b| b == "worker")
                                && man.models.iter().any(|m| m == &model)
                            {
                                if self.thrum.is_connected(cid) {
                                    found = Some(cid.clone());
                                    break;
                                } else {
                                    to_prune.push(cid.clone());
                                }
                            }
                        }
                        found
                    };
                    if !to_prune.is_empty() {
                        let mut m = self.manifests.write();
                        for cid in &to_prune { m.remove(cid); }
                    }
                    pick
                };
                let Some(worker_client) = worker_client else {
                    warn!(sid, model, "prompt.no-worker — no worker bee advertises this model");
                    let err = serde_json::json!({
                        "chi": "error",
                        "sid": sid,
                        "message": format!("no worker bee advertises model '{}'", model),
                    });
                    self.thrum.thrum_broadcast(&sid, &self.hive_tag, err);
                    return;
                };

                // Register nestler-declared tools on the MCP session so
                // the worker's MCP client sees them advertised. Dispatch
                // still routes through NestlerBridge → thrum → originator.
                if let Some(nestler_tools) = tone.get("tools").and_then(Value::as_array) {
                    let parsed: Vec<mcp::ToolDef> = nestler_tools.iter().filter_map(|t| {
                        let name = t.get("name").and_then(Value::as_str)?.to_string();
                        let description = t.get("description")
                            .and_then(Value::as_str).unwrap_or("").to_string();
                        let input_schema = t.get("parameters")
                            .or_else(|| t.get("inputSchema"))
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({"type":"object","properties":{}}));
                        Some(mcp::ToolDef { name, description, input_schema })
                    }).collect();
                    if !parsed.is_empty() {
                        let session = self.mcp_registry.session(&sid);
                        session.lock().nestler_tools = parsed;
                        trace!(sid, count = nestler_tools.len(), "mcp.nestler_tools.registered");
                    }
                }

                // Forward the prompt tone verbatim to the worker, plus
                // augment with mcpUrl + cwd if absent (workers need them).
                let mut forward = tone.clone();
                if let Some(obj) = forward.as_object_mut() {
                    if obj.get("cwd").is_none() {
                        obj.insert("cwd".into(), Value::String(cwd.clone()));
                    }
                    if obj.get("mcpUrl").is_none() {
                        obj.insert("mcpUrl".into(), Value::String(self.mcp_url.clone()));
                    }
                    if obj.get("content").is_none() && !text.is_empty() {
                        obj.insert("content".into(), Value::String(text.clone()));
                    }
                    if obj.get("systemPrompt").is_none() {
                        if let Some(sp) = system_prompt.as_ref() {
                            obj.insert("systemPrompt".into(), Value::String(sp.clone()));
                        }
                    }
                }
                trace!(sid, model, worker_client = %worker_client, "prompt.forward.to-worker");
                // Record origin so reply tones from the worker route
                // back to the originating peer humd via ensemble.
                if let Some(origin) = origin {
                    self.sid_origins.write().insert(sid.clone(), origin);
                }
                self.thrum.thrum_to(&worker_client, forward);
                self.waneman.tick(&sid);
            }
            Some(Chi::Cancel) => {
                if let Some(sid) = tone.get("sid").and_then(Value::as_str) {
                    // Forward cancel to every registered worker — they
                    // no-op on unknown sids. sid→worker routing is a
                    // future optimization.
                    let workers: Vec<String> = self.manifests.read()
                        .iter()
                        .filter(|(_, m)| m.bee.iter().any(|b| b == "worker"))
                        .map(|(cid, _)| cid.clone())
                        .collect();
                    for wc in workers {
                        self.thrum.thrum_to(&wc, tone.clone());
                    }
                    let _ = sid;
                }
            }
            Some(Chi::Cleanup) => {
                if let Some(_sid) = tone.get("sid").and_then(Value::as_str) {
                    // Forward cleanup to all registered workers.
                    let workers: Vec<String> = self.manifests.read()
                        .iter()
                        .filter(|(_, m)| m.bee.iter().any(|b| b == "worker"))
                        .map(|(cid, _)| cid.clone())
                        .collect();
                    for wc in workers {
                        self.thrum.thrum_to(&wc, tone.clone());
                    }
                    // MCP registry session drop.
                    if let Some(sid) = tone.get("sid").and_then(Value::as_str) {
                        self.mcp_registry.drop_session(sid);
                    }
                }
            }
            Some(Chi::Attach) => {
                let sid = tone.get("sid").and_then(Value::as_str).unwrap_or("").to_string();
                if sid.is_empty() {
                    warn!(client_id, "attach.no-sid");
                    return;
                }
                let hear_only = tone.get("hearOnly").and_then(Value::as_bool).unwrap_or(false);
                if client_id == "ensemble" {
                    // A peer humd is registering itself as an observer of
                    // a hum hosted here. Record so reply tones fan out.
                    let peer = tone.get("from").and_then(Value::as_str).and_then(parse_humd_id);
                    if let Some(peer) = peer {
                        let mut obs = self.observers.write();
                        let list = obs.entry(sid.clone()).or_default();
                        if !list.contains(&peer) {
                            list.push(peer);
                        }
                        trace!(client_id, sid, peer = %peer.short(), hear_only, "observer.registered");
                    } else {
                        warn!(client_id, sid, "attach.bad-from");
                    }
                } else {
                    // A local nestler is announcing it wants to observe a
                    // sid hosted on a peer. Claim the sigil so reply
                    // tones (which arrive via the ensemble pump and get
                    // broadcast on the sid) land in this client's queue,
                    // then forward the attach to the host humd.
                    self.thrum.claim_sigil(client_id, &thrum_core::sigil(&sid, &self.hive_tag));
                    self.thrum.claim_sigil(client_id, &sid);
                    if let Some(ensemble) = &self.ensemble {
                        if let Some(to) = tone.get("to").and_then(Value::as_str) {
                            if !to.is_empty() && to != ensemble.me().to_hex() {
                                trace!(client_id, sid, to, hear_only, "attach.forward");
                                let mut forward = tone.clone();
                                if let Some(obj) = forward.as_object_mut() {
                                    obj.entry("from".to_string())
                                        .or_insert_with(|| Value::String(ensemble.me().to_hex()));
                                }
                                if let Err(e) = ensemble.route(forward).await {
                                    warn!(client_id, sid, err = %e, "attach.forward.failed");
                                }
                            }
                        }
                    }
                }
            }
            Some(Chi::Detach) => {
                let sid = tone.get("sid").and_then(Value::as_str).unwrap_or("").to_string();
                if sid.is_empty() {
                    warn!(client_id, "detach.no-sid");
                    return;
                }
                if client_id == "ensemble" {
                    let peer = tone.get("from").and_then(Value::as_str).and_then(parse_humd_id);
                    if let Some(peer) = peer {
                        let mut obs = self.observers.write();
                        if let Some(list) = obs.get_mut(&sid) {
                            list.retain(|p| *p != peer);
                            if list.is_empty() { obs.remove(&sid); }
                        }
                        trace!(client_id, sid, peer = %peer.short(), "observer.removed");
                    }
                } else if let Some(ensemble) = &self.ensemble {
                    if let Some(to) = tone.get("to").and_then(Value::as_str) {
                        if !to.is_empty() && to != ensemble.me().to_hex() {
                            let mut forward = tone.clone();
                            if let Some(obj) = forward.as_object_mut() {
                                obj.entry("from".to_string())
                                    .or_insert_with(|| Value::String(ensemble.me().to_hex()));
                            }
                            if let Err(e) = ensemble.route(forward).await {
                                warn!(client_id, sid, err = %e, "detach.forward.failed");
                            }
                        }
                    }
                }
            }
            Some(Chi::PeerAdd) => {
                // Sim wires the connection into Ensemble directly; this
                // arm just records intent so peer-add tones round-trip
                // through the dispatcher for tests/logs.
                let humd_id = tone.get("humd_id").and_then(Value::as_str).unwrap_or("");
                trace!(client_id, humd_id, "ensemble.peer.add");
            }
            Some(Chi::PeerRemove) => {
                let humd_id = tone.get("humd_id").and_then(Value::as_str).unwrap_or("");
                trace!(client_id, humd_id, "ensemble.peer.remove");
                if let Some(ensemble) = &self.ensemble {
                    if let Ok(bytes) = hex::decode(humd_id) {
                        if bytes.len() == 32 {
                            let mut id = [0u8; 32];
                            id.copy_from_slice(&bytes);
                            ensemble.remove_peer(&ensemble::HumdId(id));
                        }
                    }
                }
            }
            Some(Chi::WaneSync) => {
                // Partition-heal reconciliation. Snapshot is a JSON object
                // of sigil → u64. Merge by max — wane is a Lamport clock,
                // max is the convergent join. We don't reply; both sides
                // emit on heal so each is informed exactly once.
                let snapshot = tone
                    .get("snapshot")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                let mut remote: HashMap<String, u64> = HashMap::new();
                for (sigil, v) in snapshot {
                    if let Some(n) = v.as_u64() {
                        remote.insert(sigil, n);
                    }
                }
                let advanced = self.waneman.merge(&remote);
                trace!(
                    client_id,
                    entries = remote.len(),
                    advanced,
                    "thrum.recv.wane-sync"
                );
            }
            Some(Chi::ToolResult) => {
                // Resolves a pending nestler-tool dispatch. NestlerBridge
                // is parked on a oneshot keyed by callId; pop it and
                // forward the result content. Missing callId means the
                // nestler echoed after timeout — silent drop.
                let call_id = tone.get("callId").and_then(Value::as_str);
                let result = tone.get("result").and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| tone.get("content").and_then(Value::as_str).map(str::to_string))
                    .unwrap_or_default();
                if let Some(call_id) = call_id {
                    if let Some(tx) = self.tool_pending.lock().remove(call_id) {
                        let _ = tx.send(result);
                        trace!(call_id, "tool_result.resolved");
                    } else {
                        trace!(call_id, "tool_result.no_pending");
                    }
                }
            }
            Some(Chi::Curate)
            | Some(Chi::ReleasePermit)
            | Some(Chi::TendrilResult)
            | Some(Chi::PetalCell)
            | Some(Chi::Echo)
            | Some(Chi::PerfMark)
            | Some(Chi::Log)
            | Some(Chi::Drone)
            | Some(Chi::DroneRetrofit) => {
                trace!(client_id, %chi_str, "thrum.recv.todo");
            }
            Some(other) => {
                warn!(client_id, ?other, "thrum.recv.unexpected-direction");
            }
            None => {
                warn!(client_id, %chi_str, "thrum.recv.unknown-chi");
            }
        }
    }
}

/// Capabilities the daemon advertises in the hello we send when dialling
/// a bootstrap peer. Mirrors the local nest config so peers selecting an
/// overflow target see what we can actually host. `free_slots` is left
/// `None` (unspecified / unbounded) — the overflow heuristic in
/// `pick_overflow_peer` treats `None` as "available."
fn my_capabilities(cfg: &DaemonConfig) -> PeerCapabilities {
    let nest_name = cfg.hum_cfg.nest.default.clone();
    let total_slots = cfg.hum_cfg.nest.max_procs;
    // Initial advertise: full free, Cool. Live updates come from the
    // beat path once the pool is wired (see TODO in nest::pool::Nest).
    let headroom = ensemble::headroom::CellHeadroom::from_counts(total_slots, total_slots, None);
    PeerCapabilities {
        proto_version: thrum_core::THRUM_VERSION.into(),
        nests: vec![nest_name],
        hosts: Vec::new(),
        can_relay: false,
        free_slots: Some(total_slots as usize),
        headroom,
    }
}

/// Pick a peer to forward a prompt to when local capacity is exhausted.
/// Prefers peers that advertise the requested nest kind AND claim a
/// non-zero `free_slots`. Falls back to any peer with the nest kind.
/// Returns `None` if no peer in the ensemble is eligible.
fn pick_overflow_peer(ensemble: &Ensemble, nest_kind: &str) -> Option<HumdId> {
    let peers = ensemble.peers();
    // First pass: peer with the right nest kind AND advertised slots.
    let mut fallback: Option<HumdId> = None;
    for id in peers {
        let Some(caps) = ensemble.peer_caps(&id) else { continue };
        let has_nest = caps.nests.iter().any(|n| n == nest_kind);
        if !has_nest { continue; }
        match caps.free_slots {
            Some(n) if n > 0 => return Some(id),
            None => return Some(id), // unbounded
            Some(0) => { /* peer is full, skip but remember as fallback */
                if fallback.is_none() { fallback = Some(id); }
            }
            _ => {}
        }
    }
    fallback
}

/// Parse a hex HumdId string back into the typed [`ensemble::HumdId`].
/// Returns None on bad length or non-hex characters.
fn parse_humd_id(s: &str) -> Option<ensemble::HumdId> {
    let bytes = hex::decode(s).ok()?;
    if bytes.len() != 32 { return None; }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Some(ensemble::HumdId(arr))
}

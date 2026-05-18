//! thrumd — the NDJSON unix-socket server.
//!
//! One listener, many breaths. Each connected bee gets a `Reach`: a
//! client id, an outbound channel, and a set of sigils it claims. Tones
//! arrive line-delimited over the socket; we validate the envelope (chi
//! must be a non-empty string), check for dusk, auto-echo on receipt,
//! then hand the tone to the caller's `ToneSink`.
//!
//! Outbound: callers reach in via `thrum_to(client_id, tone)` or
//! `thrum_broadcast(sid, tone)` — both are sync, lock-and-send, no await.
//! The per-connection writer task drains the channel and writes to the
//! socket; back-pressure is bounded by the channel's capacity.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use parking_lot::RwLock;
use serde_json::{json, Value};
use thrum_core::THRUM_VERSION;
use tokio::net::UnixListener;
use tracing::{info, trace, warn};

mod conn;
mod registry;

pub use registry::{ClientId, Reach};

/// A tone, on the wire, in flight. We stay loose at this layer — the
/// envelope shape is enforced (chi must be a non-empty string) but the
/// body is whatever JSON the sender packed in. Handlers can deserialize
/// into typed shapes from thrum-core when they want.
pub type Tone = Value;

/// Handler the caller plugs in. Called once per validated incoming tone,
/// after dusk check and after auto-echo. Must not block the runtime.
#[async_trait]
pub trait ToneSink: Send + Sync + 'static {
    async fn hear(&self, client_id: &str, tone: Tone);
}

/// Default socket path — `$XDG_RUNTIME_DIR/hum/thrum.sock`, or
/// `/tmp/hum/thrum.sock` if XDG_RUNTIME_DIR isn't set.
///
/// Canonical per `WIRE.md`. Env override: `HUM_THRUM_SOCK`. Legacy
/// `HUM_SOCKET` is also accepted so an in-flight upgrade doesn't break
/// already-running clients pointing at the old name — drop the
/// fallback after 0.4.
pub fn default_socket_path() -> PathBuf {
    if let Ok(p) = std::env::var("HUM_THRUM_SOCK") {
        return PathBuf::from(p);
    }
    if let Ok(p) = std::env::var("HUM_SOCKET") {
        return PathBuf::from(p);
    }
    let base = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"));
    base.join("hum").join("thrum.sock")
}

/// The thrum's living state — registry plus optional handler. Cloning is
/// cheap (Arc inside). Hand a clone to spawned tasks; keep one in main.
#[derive(Clone)]
pub struct Thrum {
    inner: Arc<ThrumInner>,
}

struct ThrumInner {
    clients: RwLock<registry::Registry>,
    sink: RwLock<Option<Arc<dyn ToneSink>>>,
}

impl Thrum {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ThrumInner {
                clients: RwLock::new(registry::Registry::new()),
                sink: RwLock::new(None),
            }),
        }
    }

    /// Install the tone sink. Replaces any previous one.
    pub fn set_sink(&self, sink: Arc<dyn ToneSink>) {
        *self.inner.sink.write() = Some(sink);
    }

    /// True if a sink is currently installed.
    pub fn has_sink(&self) -> bool {
        self.inner.sink.read().is_some()
    }

    /// True if the given client_id still has a live thrum connection
    /// (either a real UDS conn or a synthetic registration). Lets the
    /// host prune stale registry entries lazily on access.
    pub fn is_connected(&self, client_id: &str) -> bool {
        self.inner.clients.read().get(client_id).is_some()
    }

    /// Send to one specific client. Drops silently if the client is gone
    /// or its outbound queue is full — callers can probe with `has_client`.
    pub fn thrum_to(&self, client_id: &str, tone: Tone) {
        let reg = self.inner.clients.read();
        if let Some(reach) = reg.get(client_id) {
            let chi = chi_of(&tone).unwrap_or("?").to_string();
            if reach.send(tone).is_err() {
                trace!(client_id = %short(client_id), chi = %chi, "thrum.send.dropped");
            }
        } else {
            trace!(client_id = %short(client_id), "thrum.send.unknown-client");
        }
    }

    /// Send to every client claiming the given sid's sigil. Falls back to
    /// every unregistered client (no sigils claimed) if nobody owns it —
    /// matches the TS daemon's routing behaviour. `nest` selects which
    /// nest-kind namespace to compute the sigil under (e.g. "claude-cli",
    /// "claude-repl", future nests).
    pub fn thrum_broadcast(&self, sid: &str, nest: &str, mut tone: Tone) {
        let sigil = thrum_core::sigil(sid, nest);
        if let Some(obj) = tone.as_object_mut() {
            obj.entry("sid").or_insert(json!(sid));
            obj.entry("sigil").or_insert(json!(sigil));
            obj.insert("from".into(), json!("daemon"));
        }
        let reg = self.inner.clients.read();
        let mut sent = false;
        for reach in reg.iter() {
            if reach.has_sigil(&sigil) {
                let _ = reach.send(tone.clone());
                sent = true;
            }
        }
        if !sent {
            for reach in reg.iter() {
                if reach.sigil_count() == 0 {
                    let _ = reach.send(tone.clone());
                }
            }
        }
        trace!(
            sid = %sid,
            chi = %chi_of(&tone).unwrap_or("?"),
            sent,
            "thrum.broadcast"
        );
    }

    /// Send to every connected client, regardless of sigils.
    pub fn thrum_all(&self, tone: Tone) {
        let reg = self.inner.clients.read();
        for reach in reg.iter() {
            let _ = reach.send(tone.clone());
        }
    }

    /// Claim a sigil for a client — call from your `ToneSink::hear` when
    /// you observe a `hello` (or any chi that establishes ownership).
    pub fn claim_sigil(&self, client_id: &str, sigil: impl Into<String>) {
        let reg = self.inner.clients.read();
        if let Some(reach) = reg.get(client_id) {
            reach.add_sigil(sigil.into());
        }
    }

    pub fn has_client(&self, client_id: &str) -> bool {
        self.inner.clients.read().get(client_id).is_some()
    }

    pub fn client_count(&self) -> usize {
        self.inner.clients.read().len()
    }

    /// Register a synthetic in-process client. The returned receiver
    /// drains outbound tones bound for this client; `client_id` becomes
    /// addressable via [`thrum_to`] and [`thrum_broadcast`]. Used by
    /// sim to fake a connected nestler without a socket.
    pub fn register_synthetic(&self, client_id: impl Into<String>) -> tokio::sync::mpsc::Receiver<Value> {
        let cid: String = client_id.into();
        let (reach, rx) = registry::Reach::new(cid);
        let mut reg = self.inner.clients.write();
        reg.insert(Arc::new(reach));
        rx
    }

    /// Drop a previously-registered synthetic client.
    pub fn drop_synthetic(&self, client_id: &str) {
        self.inner.clients.write().remove(client_id);
    }

    /// Inject a tone as if it arrived from `client_id`. Bypasses the
    /// NDJSON socket and goes straight to the installed sink. No
    /// envelope validation, no auto-echo — sim drives the shape it
    /// wants. The sink must be installed beforehand.
    pub async fn inject_tone(&self, client_id: &str, tone: Tone) {
        let sink = self.inner.sink.read().clone();
        if let Some(sink) = sink {
            sink.hear(client_id, tone).await;
        } else {
            trace!(client_id = %short(client_id), "thrum.inject.no-sink");
        }
    }
}

impl Default for Thrum {
    fn default() -> Self {
        Self::new()
    }
}

/// Bind the unix socket and run the accept loop forever. Removes any
/// stale socket file at `path` before binding. Returns on listener error.
pub async fn serve(thrum: Thrum, path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create socket dir {:?}", parent))?;
    }
    if path.exists() {
        std::fs::remove_file(path).with_context(|| format!("remove stale socket {:?}", path))?;
    }
    let listener = UnixListener::bind(path)
        .with_context(|| format!("bind unix socket {:?}", path))?;
    info!(path = %path.display(), version = %THRUM_VERSION, "thrum.listening");

    loop {
        match listener.accept().await {
            Ok((sock, _)) => {
                let thrum = thrum.clone();
                tokio::spawn(async move {
                    conn::run(thrum, sock).await;
                });
            }
            Err(e) => {
                warn!(err = %e, "thrum.accept.failed");
            }
        }
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

pub(crate) fn chi_of(tone: &Value) -> Option<&str> {
    tone.get("chi").and_then(|v| v.as_str())
}

pub(crate) fn rid_of(tone: &Value) -> Option<&str> {
    tone.get("rid").and_then(|v| v.as_str())
}

pub(crate) fn short(id: &str) -> String {
    id.chars().take(8).collect()
}

/// Validate envelope: chi must be a present, non-empty string. Returns
/// None on success, Some(reason) on rejection — caller traces and drops.
pub(crate) fn validate_envelope(tone: &Value) -> Option<&'static str> {
    let Some(obj) = tone.as_object() else {
        return Some("not-object");
    };
    let Some(chi) = obj.get("chi") else {
        return Some("no-chi");
    };
    let Some(s) = chi.as_str() else {
        return Some("chi-not-string");
    };
    if s.is_empty() {
        return Some("chi-empty");
    }
    None
}

/// Wrap thrum-core's is_dusk — it expects a typed Tone we don't yet have,
/// so we re-implement against the loose Value envelope until thrum-core
/// exposes a JSON-friendly variant. Behaviour: dusk is `now_ms` past the
/// `dusk` field if present and numeric.
pub(crate) fn tone_is_dusk(tone: &Value) -> bool {
    // Prefer thrum-core's primitive when the shape lines up; otherwise
    // walk the value ourselves. Cheap either way.
    if let Some(dusk) = tone.get("dusk").and_then(|v| v.as_u64()) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        return now > dusk;
    }
    false
}

/// Build the breath handshake tone — daemon stamps its sessions view and
/// protoVersion. Per-session shape is opaque here; callers compose it.
pub fn breath_tone(sessions: Value) -> Tone {
    json!({
        "chi": "breath",
        "from": "daemon",
        "sessions": sessions,
        "protoVersion": THRUM_VERSION,
    })
}

/// Build the echo tone for `rid`.
pub fn echo_tone(rid: &str, ok: bool, error: Option<&str>) -> Tone {
    let mut v = json!({ "chi": "echo", "rid": rid, "ok": ok });
    if let Some(e) = error {
        v.as_object_mut().unwrap().insert("error".into(), json!(e));
    }
    v
}

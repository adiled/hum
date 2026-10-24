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
//!
//! Delivery respects the chi scope a client declared in its hello. A
//! client that declared nothing accepts every chi — see
//! [`registry::Reach::accepts_chi`].
//!
//! The envelope is loose here: `chi` must be a present, non-empty
//! string, and the body is whatever JSON the sender packed in. Handlers
//! deserialize into typed shapes from thrum-core when they want.
//!
//! [`ToneSink::forget`] fires when a client's connection drops. The
//! sink should release per-client state keyed by `client_id` — humd
//! evicts the bee's manifest there, so its tools stop being advertised
//! the instant it disconnects rather than lingering until a same-hid
//! re-hello.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use governor::{Quota, RateLimiter};
use parking_lot::RwLock;
use serde_json::{Value, json};
use thrum_core::THRUM_VERSION;
use tokio::net::UnixListener;
use tracing::{info, trace, warn};

mod audit;
mod conn;
mod registry;

pub use audit::WireCheck;

pub use registry::{ClientId, Reach};

pub type Tone = Value;

#[async_trait]
pub trait ToneSink: Send + Sync + 'static {
    async fn hear(&self, client_id: &str, tone: Tone);

    async fn forget(&self, _client_id: &str) {}
}

pub fn default_socket_path() -> PathBuf {
    hum_paths::thrum_sock()
}

#[derive(Clone)]
pub struct Thrum {
    inner: Arc<ThrumInner>,
}

struct ThrumInner {
    clients: RwLock<registry::Registry>,
    sink: RwLock<Option<Arc<dyn ToneSink>>>,
    audit: Arc<audit::Audit>,
}

impl Thrum {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ThrumInner {
                clients: RwLock::new(registry::Registry::new()),
                sink: RwLock::new(None),
                audit: Arc::default(),
            }),
        }
    }

    pub fn set_sink(&self, sink: Arc<dyn ToneSink>) {
        *self.inner.sink.write() = Some(sink);
    }

    pub fn set_wire_check(&self, mode: WireCheck) {
        self.inner.audit.set(mode);
    }

    pub fn wire_strays(&self) -> Vec<String> {
        self.inner.audit.strays()
    }

    pub(crate) fn reach(&self, client_id: String) -> (registry::Reach, tokio::sync::mpsc::Receiver<Value>) {
        registry::Reach::audited(client_id, self.inner.audit.clone())
    }

    pub(crate) fn heard(&self, client_id: &str, tone: &Value) {
        self.inner.audit.heard(client_id, tone);
    }

    pub fn has_sink(&self) -> bool {
        self.inner.sink.read().is_some()
    }

    pub fn is_connected(&self, client_id: &str) -> bool {
        self.inner.clients.read().get(client_id).is_some()
    }

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

    pub fn thrum_broadcast(&self, sid: &str, mut tone: Tone) {
        let sigil = thrum_core::sigil(sid);
        if let Some(obj) = tone.as_object_mut() {
            obj.entry("sid").or_insert(json!(sid));
            obj.entry("sigil").or_insert(json!(sigil));
            obj.insert("from".into(), json!("daemon"));
        }
        let chi = chi_of(&tone).unwrap_or("").to_string();
        let reg = self.inner.clients.read();
        let mut sent = false;
        for reach in reg.iter() {
            if reach.has_sigil(&sigil) && reach.accepts_chi(&chi) {
                let _ = reach.send(tone.clone());
                sent = true;
            }
        }
        if !sent {
            for reach in reg.iter() {
                if reach.sigil_count() == 0 && reach.accepts_chi(&chi) {
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

    pub fn thrum_all(&self, tone: Tone) {
        let chi = chi_of(&tone).unwrap_or("").to_string();
        let reg = self.inner.clients.read();
        for reach in reg.iter() {
            if reach.accepts_chi(&chi) {
                let _ = reach.send(tone.clone());
            }
        }
    }

    pub fn claim_sigil(&self, client_id: &str, sigil: impl Into<String>) {
        let reg = self.inner.clients.read();
        if let Some(reach) = reg.get(client_id) {
            reach.add_sigil(sigil.into());
        }
    }

    pub fn claim_chis(&self, client_id: &str, chis: Vec<String>) {
        let reg = self.inner.clients.read();
        if let Some(reach) = reg.get(client_id) {
            reach.replace_chis(chis);
        }
    }

    pub fn has_client(&self, client_id: &str) -> bool {
        self.inner.clients.read().get(client_id).is_some()
    }

    pub fn client_count(&self) -> usize {
        self.inner.clients.read().len()
    }

    pub fn register_synthetic(
        &self,
        client_id: impl Into<String>,
    ) -> tokio::sync::mpsc::Receiver<Value> {
        let cid: String = client_id.into();
        let (reach, rx) = self.reach(cid);
        let mut reg = self.inner.clients.write();
        reg.insert(Arc::new(reach));
        rx
    }

    pub fn drop_synthetic(&self, client_id: &str) {
        self.inner.clients.write().remove(client_id);
    }

    pub fn thrum_close(&self, client_id: &str) {
        self.inner.clients.write().remove(client_id);
    }

    pub async fn inject_tone(&self, client_id: &str, tone: Tone) {
        self.heard(client_id, &tone);
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

pub async fn serve(thrum: Thrum, path: impl AsRef<Path>) -> Result<()> {
    serve_with_hook(thrum, path, |_| {}).await
}

pub async fn serve_with_hook<F>(thrum: Thrum, path: impl AsRef<Path>, on_bound: F) -> Result<()>
where
    F: FnOnce(&Path) + Send + 'static,
{
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create socket dir {:?}", parent))?;
    }
    if path.exists() {
        std::fs::remove_file(path).with_context(|| format!("remove stale socket {:?}", path))?;
    }
    let listener =
        UnixListener::bind(path).with_context(|| format!("bind unix socket {:?}", path))?;
    info!(path = %path.display(), version = %THRUM_VERSION, "thrum.listening");
    on_bound(path);

    let limiter = RateLimiter::direct(Quota::per_second(std::num::NonZeroU32::new(100).unwrap()));

    loop {
        limiter.until_ready().await;
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


pub(crate) fn chi_of(tone: &Value) -> Option<&str> {
    tone.get("chi").and_then(|v| v.as_str())
}

pub(crate) fn rid_of(tone: &Value) -> Option<&str> {
    tone.get("rid").and_then(|v| v.as_str())
}

pub(crate) fn short(id: &str) -> String {
    id.chars().take(8).collect()
}

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

pub(crate) fn tone_is_dusk(tone: &Value) -> bool {
    if let Some(dusk) = tone.get("dusk").and_then(|v| v.as_u64()) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        return now > dusk;
    }
    false
}

pub fn breath_tone(sessions: Value) -> Tone {
    json!({
        "chi": "breath",
        "from": "daemon",
        "sessions": sessions,
        "protoVersion": THRUM_VERSION,
    })
}

pub fn echo_tone(rid: &str, ok: bool, error: Option<&str>) -> Tone {
    let mut v = json!({ "chi": "echo", "rid": rid, "ok": ok });
    if let Some(e) = error {
        v.as_object_mut().unwrap().insert("error".into(), json!(e));
    }
    v
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtoVerdict {
    Same,
    SameMajor,
    Incompatible,
}

fn parse_proto(v: &str) -> Option<(u64, u64, u64)> {
    let mut it = v.trim().split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let patch = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    Some((major, minor, patch))
}

pub fn proto_verdict(declared: &str, current: &str) -> ProtoVerdict {
    match (parse_proto(declared), parse_proto(current)) {
        (Some(d), Some(c)) if d == c => ProtoVerdict::Same,
        (Some(d), Some(c)) if d.0 == c.0 => ProtoVerdict::SameMajor,
        _ => ProtoVerdict::Incompatible,
    }
}

#[cfg(test)]
mod proto_tests {
    use super::*;

    #[test]
    fn an_exact_match_is_same() {
        assert_eq!(proto_verdict("0.7.0", "0.7.0"), ProtoVerdict::Same);
    }

    #[test]
    fn drift_inside_a_major_is_admitted() {
        assert_eq!(proto_verdict("0.6.2", "0.7.0"), ProtoVerdict::SameMajor);
        assert_eq!(proto_verdict("0.9.9", "0.7.0"), ProtoVerdict::SameMajor);
        assert_eq!(proto_verdict("0.7", "0.7.0"), ProtoVerdict::Same);
    }

    #[test]
    fn a_different_major_is_incompatible() {
        assert_eq!(proto_verdict("1.0.0", "0.7.0"), ProtoVerdict::Incompatible);
        assert_eq!(proto_verdict("0.7.0", "1.0.0"), ProtoVerdict::Incompatible);
    }

    #[test]
    fn an_unparseable_declaration_is_incompatible() {
        assert_eq!(proto_verdict("banana", "0.7.0"), ProtoVerdict::Incompatible);
        assert_eq!(proto_verdict("", "0.7.0"), ProtoVerdict::Incompatible);
        assert_eq!(proto_verdict("x.y.z", "0.7.0"), ProtoVerdict::Incompatible);
    }
}

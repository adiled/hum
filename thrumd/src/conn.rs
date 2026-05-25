//! Per-connection task: reader splits NDJSON, writer drains the outbound
//! channel. On hello we record the announcement; on every valid tone we
//! auto-echo (unless chi is echo/log) then hand to the sink.

use std::sync::Arc;

use serde_json::{json, Value};
use thrum_core::THRUM_VERSION;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tracing::{info, trace, warn};
use uuid::Uuid;

use crate::registry::Reach;
use crate::{
    breath_tone, chi_of, echo_tone, rid_of, short, tone_is_dusk, validate_envelope, Thrum,
};

pub async fn run(thrum: Thrum, sock: UnixStream) {
    let client_id = Uuid::new_v4().to_string();
    let (reach, rx) = Reach::new(client_id.clone());
    let reach = Arc::new(reach);

    {
        let mut reg = thrum.inner_clients_write();
        reg.insert(reach.clone());
        let total = reg.len();
        info!(client_id = %short(&client_id), total, "thrum.connected");
    }

    // Split the stream so reader + writer can run concurrently without
    // fighting over &mut self. tokio::io::split is the standard tool.
    let (read_half, write_half) = sock.into_split();

    // Outbound: drain the channel, write each tone + '\n' to the socket.
    // Exits when the channel closes (Reach dropped from registry).
    let writer_task = tokio::spawn(async move {
        write_loop(write_half, rx).await;
    });

    // First message we send: the breath. Sessions list is empty here —
    // callers can immediately follow up with a richer breath via
    // `thrum_to` once they observe the new client.
    let _ = reach.send(breath_tone(json!([])));
    trace!(client_id = %short(&client_id), version = %THRUM_VERSION, "thrum.breath.sent");

    // Reader loop.
    read_loop(&thrum, &client_id, &reach, read_half).await;

    // Reader exited (EOF/error) — pull from registry and let writer drain.
    {
        let mut reg = thrum.inner_clients_write();
        reg.remove(&client_id);
        let total = reg.len();
        info!(client_id = %short(&client_id), total, "thrum.disconnected");
    }
    // Let the sink release per-client state (humd evicts the bee's
    // manifest here so tools stop being advertised on disconnect).
    if let Some(sink) = thrum.inner_sink() {
        sink.forget(&client_id).await;
    }
    drop(reach);
    let _ = writer_task.await;
}

async fn read_loop(
    thrum: &Thrum,
    client_id: &str,
    reach: &Arc<Reach>,
    read_half: tokio::net::unix::OwnedReadHalf,
) {
    let mut lines = BufReader::new(read_half).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(l)) => l,
            Ok(None) => break, // EOF
            Err(e) => {
                trace!(client_id = %short(client_id), err = %e, "thrum.read.failed");
                break;
            }
        };
        if line.is_empty() {
            continue;
        }
        let tone: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                trace!(client_id = %short(client_id), err = %e, "thrum.parse.failed");
                continue;
            }
        };
        dispatch(thrum, client_id, reach, tone).await;
    }
}

async fn dispatch(thrum: &Thrum, client_id: &str, reach: &Arc<Reach>, tone: Value) {
    // Envelope validation — chi must be a non-empty string.
    if let Some(reason) = validate_envelope(&tone) {
        trace!(client_id = %short(client_id), reason, "thrum.tone.rejected");
        return;
    }
    let chi = chi_of(&tone).unwrap_or("").to_string();
    let rid = rid_of(&tone).map(|s| s.to_string());

    if chi != "log" {
        trace!(
            client_id = %short(client_id),
            chi = %chi,
            rid = rid.as_deref().unwrap_or(""),
            "thrum.tone.received"
        );
    }

    // Dusk: tones past their expiry get a not-ok echo and are dropped.
    if tone_is_dusk(&tone) {
        trace!(chi = %chi, rid = rid.as_deref().unwrap_or(""), "thrum.tone.dusk");
        if let Some(rid) = rid.as_deref() {
            let _ = reach.send(echo_tone(rid, false, Some("past dusk")));
        }
        return;
    }

    // Auto-echo on receipt — skip self-echoes and chatty log frames.
    if chi != "echo" && chi != "log" {
        if let Some(rid) = rid.as_deref() {
            let _ = reach.send(echo_tone(rid, true, None));
        }
    }

    // Hello: capture nestler identity + version skew. Doesn't claim a
    // sigil by itself — the sink does that when it understands which sid
    // this client is responsible for.
    if chi == "hello" {
        let bee = tone
            .get("bee")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        let proto_version = tone
            .get("protoVersion")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        info!(
            client_id = %short(client_id),
            bee = %bee,
            proto_version = %proto_version,
            "thrum.hello"
        );
        if !proto_version.is_empty() && proto_version != THRUM_VERSION {
            warn!(
                client_id = %short(client_id),
                daemon = %THRUM_VERSION,
                nestler = %proto_version,
                "thrum.version.mismatch"
            );
        }
    }

    // Hand to the sink (if installed). Errors and panics inside the sink
    // are the sink's problem — we keep reading either way.
    let sink = thrum.inner_sink();
    if let Some(sink) = sink {
        sink.hear(client_id, tone).await;
    } else {
        trace!(chi = %chi, "thrum.tone.no-sink");
    }
}

async fn write_loop(
    mut write_half: tokio::net::unix::OwnedWriteHalf,
    mut rx: mpsc::Receiver<Value>,
) {
    while let Some(tone) = rx.recv().await {
        let mut line = match serde_json::to_vec(&tone) {
            Ok(b) => b,
            Err(e) => {
                trace!(err = %e, "thrum.write.serialize-failed");
                continue;
            }
        };
        line.push(b'\n');
        if let Err(e) = write_half.write_all(&line).await {
            trace!(err = %e, "thrum.write.failed");
            break;
        }
    }
    let _ = write_half.shutdown().await;
}

// ── Thrum private accessors ─────────────────────────────────────────────────
// Defined here as inherent methods on Thrum via a pub(crate) impl, keeping
// the lib.rs surface clean.

impl Thrum {
    pub(crate) fn inner_clients_write(
        &self,
    ) -> parking_lot::RwLockWriteGuard<'_, crate::registry::Registry> {
        self.inner.clients.write()
    }

    pub(crate) fn inner_sink(&self) -> Option<Arc<dyn crate::ToneSink>> {
        self.inner.sink.read().clone()
    }
}

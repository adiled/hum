//! `grpc-bee` — transport-only bridge from gRPC bidi stream to thrum.
//!
//! One service, one RPC: `Stream(stream Tone) returns (stream Tone)`.
//! Every tone humd emits flows back over gRPC; every tone the gRPC client
//! sends is forwarded to humd. Nothing is translated — gRPC is the
//! transport, thrum is still the protocol.
//!
//! Each bidi stream opens its own thrum connection so concurrent gRPC
//! clients can use overlapping sids without colliding handler state.

use std::pin::Pin;

use anyhow::Result;
use serde_json::Value;
use thrum_core::{Chi, THRUM_VERSION};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt};
use tonic::{transport::Server, Request, Response, Status, Streaming};
use tracing::{info, warn};

pub mod hum {
    tonic::include_proto!("hum");
}

use hum::{hum_server::{Hum, HumServer}, Tone};

const HIVE_NAME: &str = "grpc";
const NESTLING_VERSION: &str = env!("CARGO_PKG_VERSION");

fn humd_sock_path() -> String {
    if let Ok(s) = std::env::var("HUM_THRUM_SOCK") {
        return s;
    }
    let runtime = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| format!("/run/user/{}", unsafe { libc::geteuid() }));
    format!("{runtime}/hum/thrum.sock")
}

/// One bidi stream's bridge: open a thrum connection, pump tones both ways.
async fn bridge(
    mut incoming: Streaming<Tone>,
    out: mpsc::Sender<Result<Tone, Status>>,
) -> Result<()> {
    let sock = UnixStream::connect(humd_sock_path()).await?;
    let (rd, mut wr) = sock.into_split();
    let mut lines = BufReader::new(rd).lines();

    // Persisted forager identity — humd dedupes us by this fbee_ hid
    // across reconnects; without it every reconnect leaks a manifest.
    let hid = nest_common::load_or_mint_bee_key(HIVE_NAME, ensemble::HidPrefix::Fbee)
        .map(|k| k.hid.to_hex())
        .unwrap_or_default();

    // Send hello on connect so humd advertises us to the mesh.
    let hello = serde_json::json!({
        "chi": Chi::Hello,
        "rid": format!("hello-{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis().to_string())
            .unwrap_or_default()),
        "from": HIVE_NAME,
        "hid": hid,
        "bee": ["forager"],
        "version": NESTLING_VERSION,
        "protoVersion": THRUM_VERSION,
        "propensity": {
            "statefulness": "transport-only",
            "richness": "opaque",
            "wire": "grpc/bidi"
        },
        "source": "https://github.com/adiled/hum/tree/main/hives/grpc"
    });
    let mut buf = serde_json::to_string(&hello)?;
    buf.push('\n');
    wr.write_all(buf.as_bytes()).await?;

    // gRPC → thrum
    let to_thrum = tokio::spawn(async move {
        while let Some(item) = incoming.next().await {
            let wire = match item {
                Ok(t) => t,
                Err(e) => {
                    warn!(error = %e, "grpc.recv");
                    break;
                }
            };
            // Body is authoritative — full JSON tone. Fall back to
            // (chi, sid, rid) if body is empty (older clients).
            let line = if !wire.body.is_empty() {
                match String::from_utf8(wire.body.clone()) {
                    Ok(s) => s,
                    Err(_) => continue,
                }
            } else {
                serde_json::to_string(&serde_json::json!({
                    "chi": wire.chi,
                    "sid": wire.sid,
                    "rid": wire.rid,
                }))
                .unwrap_or_default()
            };
            let mut framed = line;
            framed.push('\n');
            if wr.write_all(framed.as_bytes()).await.is_err() {
                break;
            }
        }
    });

    // thrum → gRPC
    while let Some(line) = lines.next_line().await? {
        if line.is_empty() {
            continue;
        }
        let tone: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "thrum.parse");
                continue;
            }
        };
        let wire = Tone {
            chi: tone.get("chi").and_then(Value::as_str).unwrap_or("").into(),
            sid: tone.get("sid").and_then(Value::as_str).unwrap_or("").into(),
            rid: tone.get("rid").and_then(Value::as_str).unwrap_or("").into(),
            body: line.into_bytes(),
        };
        if out.send(Ok(wire)).await.is_err() {
            break;
        }
    }
    let _ = to_thrum.await;
    Ok(())
}

#[derive(Default)]
struct HumBridge;

#[tonic::async_trait]
impl Hum for HumBridge {
    type StreamStream = Pin<Box<dyn Stream<Item = Result<Tone, Status>> + Send + 'static>>;

    async fn stream(
        &self,
        req: Request<Streaming<Tone>>,
    ) -> Result<Response<Self::StreamStream>, Status> {
        let incoming = req.into_inner();
        let (tx, rx) = mpsc::channel::<Result<Tone, Status>>(64);
        tokio::spawn(async move {
            if let Err(e) = bridge(incoming, tx).await {
                warn!(error = ?e, "bridge.exit");
            }
        });
        let stream = ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(stream) as Self::StreamStream))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let host = std::env::var("HUM_GRPC_HOST").unwrap_or_else(|_| "0.0.0.0".into());
    let port = std::env::var("HUM_GRPC_PORT").unwrap_or_else(|_| "14621".into());
    let addr = format!("{host}:{port}").parse()?;

    info!(addr = %addr, "grpc-bee.listen");
    Server::builder()
        .add_service(HumServer::new(HumBridge::default()))
        .serve(addr)
        .await?;
    Ok(())
}

//! `grpc-bee` — transport-only bridge from gRPC bidi stream to thrum.
//!
//! One service, one RPC: `Stream(stream Tone) returns (stream Tone)`.
//! Every tone humd emits flows back over gRPC; every tone the gRPC client
//! sends is forwarded to humd. Nothing is translated — gRPC is the
//! transport, thrum is still the protocol.
//!
//! Each bidi stream links its own bee connection through hum's client, so
//! concurrent gRPC clients can use overlapping sids without colliding
//! handler state.

use std::pin::Pin;

use anyhow::{Context, Result};
use hum_thrum::{Bee, Event, Options, Refused};
use serde_json::Value;
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

async fn bridge(
    mut incoming: Streaming<Tone>,
    out: mpsc::Sender<Result<Tone, Status>>,
) -> Result<()> {
    let hello = serde_json::json!({
        "version": NESTLING_VERSION,
        "propensity": {
            "statefulness": "transport-only",
            "richness": "opaque",
            "wire": "grpc/bidi"
        },
        "source": "https://github.com/adiled/hum/tree/main/hives/grpc"
    });
    let mut bee = Bee::link(Options::new(HIVE_NAME, "main", thrum_core::Role::Forager).hello(hello))
        .await
        .context("link the grpc bee")?;
    let sender = bee.sender();

    let to_thrum = tokio::spawn(async move {
        while let Some(item) = incoming.next().await {
            let wire = match item {
                Ok(t) => t,
                Err(e) => {
                    warn!(error = %e, "grpc.recv");
                    break;
                }
            };
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
            if let Ok(tone) = serde_json::from_str::<Value>(&line) {
                sender.send(tone);
            }
        }
        sender.close();
    });

    while let Some(event) = bee.next().await {
        match event {
            Event::Tone(tone) => {
                let wire = Tone {
                    chi: tone.get("chi").and_then(Value::as_str).unwrap_or("").into(),
                    sid: tone.get("sid").and_then(Value::as_str).unwrap_or("").into(),
                    rid: tone.get("rid").and_then(Value::as_str).unwrap_or("").into(),
                    body: tone.to_string().into_bytes(),
                };
                if out.send(Ok(wire)).await.is_err() {
                    break;
                }
            }
            Event::Refused(why) => {
                to_thrum.abort();
                return Err(Refused(why).into());
            }
            Event::Nestled | Event::Astray | Event::Advisory(_) => {}
        }
    }
    bee.close();
    to_thrum.abort();
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
    hum_paths::init();
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

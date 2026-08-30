//! `hum-thrum` — the thrum wire client for bees.
//!
//! A bee is a kind + a binary. Its *runtime* only needs the wire:
//! dial the Unix socket humd binds, send a `chi:"hello"`, read NDJSON
//! tones, ship results, and reconnect forever. It does **not** need
//! the daemon's in-memory nest, drone, or ensemble machinery.
//!
//! This crate factors the wire loop out of `serve_worker` /
//! `serve_forager` into a transport-agnostic seam, so a remote hive
//! can depend on it without pulling the daemon tree:
//!
//! - [`connect`] — dial + split the socket, returning a write half.
//! - [`send_json`] — write one NDJSON line (a tone).
//! - [`read_tones`] — read NDJSON lines, parse each into a [`Value`],
//!   and hand it to a caller-supplied per-chi dispatcher.
//! - [`serve_forever`] — the reconnect loop with jittered backoff.
//!
//! The chi *semantics* (what a `tool-call`, `prompt`, or `cancel`
//! actually does) stay in the caller — that's worker/forager-specific
//! state. What's shared is the wire mechanics.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use tracing::{trace, warn};

/// Dial the thrum Unix socket and split it. Returns the read half as
/// a line reader and the write half wrapped in a shared mutex (so
/// concurrent spawns can write).
pub async fn connect(
    path: &Path,
) -> Result<(Lines<BufReader<tokio::net::unix::OwnedReadHalf>>, Arc<Mutex<tokio::net::unix::OwnedWriteHalf>>)> {
    let stream = UnixStream::connect(path).await
        .with_context(|| format!("connect to thrum at {}", path.display()))?;
    let (read_half, write_half) = stream.into_split();
    let write_half = Arc::new(Mutex::new(write_half));
    let reader = BufReader::new(read_half).lines();
    Ok((reader, write_half))
}

/// Write one NDJSON line (a tone) to the shared write half.
pub async fn send_json(write: &Arc<Mutex<tokio::net::unix::OwnedWriteHalf>>, tone: &Value) -> Result<()> {
    let line = format!("{}\n", tone);
    write.lock().await.write_all(line.as_bytes()).await?;
    Ok(())
}

/// Read NDJSON tones and hand each to `on_tone`. Returns `Ok(())`
/// when the stream ends (peer closed — caller reconnects).
pub async fn read_tones<F>(mut reader: Lines<BufReader<tokio::net::unix::OwnedReadHalf>>, on_tone: F) -> Result<()>
where
    F: Fn(Value),
{
    while let Some(line) = reader.next_line().await? {
        if line.is_empty() { continue; }
        let tone: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => { trace!(err = %e, "thrum.parse.skip"); continue; }
        };
        on_tone(tone);
    }
    Ok(())
}

/// The reconnect loop. `dial` is called each attempt (it should
/// connect, hello, and run the tone loop), returning `Ok(())` on a
/// clean exit or `Err` on a failed connection. Sleeps with jittered
/// backoff between attempts so parallel boot races with humd stay
/// quiet (matching serve_worker's grace window).
pub async fn serve_forever<F, Fut>(dial: F) -> !
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let mut consecutive_fails = 0u32;
    loop {
        match dial().await {
            Ok(()) => {
                consecutive_fails = 0;
                trace!("thrum: clean exit, reconnecting");
            }
            Err(e) => {
                consecutive_fails += 1;
                warn!(err = %e, attempts = consecutive_fails, "thrum: connection failed, retrying");
            }
        }
        let jitter = rand::random::<f32>() * 0.75;
        tokio::time::sleep(std::time::Duration::from_secs_f32(2.0 + jitter)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::net::UnixListener;
    use tokio::io::{AsyncBufReadExt, BufReader};

    // Wire round-trip: a server accepts a Unix connection, the client
    // dials via hum-thrum::connect, sends a hello tone, and the server
    // reads it back. Confirms connect/send_json/read_tones line up with
    // the NDJSON contract humd speaks.
    #[tokio::test]
    async fn wire_round_trip() {
        let dir = std::env::temp_dir().join(format!("hum-thrum-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("thrum.sock");
        let listener = UnixListener::bind(&sock).unwrap();

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = String::new();
            let mut r = BufReader::new(&mut stream);
            r.read_line(&mut buf).await.unwrap();
            let tone: Value = serde_json::from_str(&buf).unwrap();
            assert_eq!(tone["chi"], "hello");
            assert_eq!(tone["bee"][0], "forager");
        });

        let (_, write) = connect(&sock).await.unwrap();
        let hello = json!({ "chi": "hello", "bee": ["forager"], "hid": "fbee_abcd" });
        send_json(&write, &hello).await.unwrap();

        server.await.unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

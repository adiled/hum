//! Integration smoke test for the thrum wire.
//!
//! Connects to a running humd-rs daemon over its UNIX socket, sends a
//! `chi:"hello"` tone, reads back the `chi:"breath"` greeting, and asserts
//! `protoVersion == "0.2.0"`. First proof the Rust daemon's wire is alive.
//!
//! Run: `cargo run --example smoke -p humd-bin`
//!
//! Socket path: `$HUM_THRUM_SOCK` if set, else `$XDG_STATE_HOME/hum/thrum.sock`.
//! Legacy `HUM_SOCKET` also accepted.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::timeout;

const EXPECTED_PROTO: &str = "0.3.0";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const READ_TIMEOUT: Duration = Duration::from_secs(5);

fn socket_path() -> PathBuf {
    hum_paths::thrum_sock_resolved()
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    hum_paths::init();
    let path = socket_path();

    let stream = match timeout(CONNECT_TIMEOUT, UnixStream::connect(&path)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            eprintln!("DAEMON DOWN: connect {} failed: {}", path.display(), e);
            return ExitCode::from(1);
        }
        Err(_) => {
            eprintln!("DAEMON DOWN: connect {} timed out", path.display());
            return ExitCode::from(1);
        }
    };

    let (rd, mut wr) = stream.into_split();
    let mut reader = BufReader::new(rd);

    let hello = json!({ "chi": "hello" });
    let mut line = serde_json::to_vec(&hello).expect("serialize hello");
    line.push(b'\n');
    if let Err(e) = wr.write_all(&line).await {
        panic!("write hello failed: {e}");
    }
    if let Err(e) = wr.flush().await {
        panic!("flush hello failed: {e}");
    }

    let mut buf = String::new();
    match timeout(READ_TIMEOUT, reader.read_line(&mut buf)).await {
        Ok(Ok(0)) => panic!("daemon closed socket before sending breath"),
        Ok(Ok(_)) => {}
        Ok(Err(e)) => panic!("read breath failed: {e}"),
        Err(_) => panic!("timeout waiting for breath from {}", path.display()),
    }

    let tone: Value = serde_json::from_str(buf.trim())
        .unwrap_or_else(|e| panic!("invalid JSON breath: {e}: {buf:?}"));

    let chi = tone.get("chi").and_then(Value::as_str).unwrap_or_else(|| {
        panic!("breath tone missing `chi` field: {tone}");
    });
    if chi != "breath" {
        panic!("expected chi=\"breath\", got chi={chi:?}: {tone}");
    }

    let proto = tone
        .get("protoVersion")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("breath tone missing `protoVersion`: {tone}"));
    if proto != EXPECTED_PROTO {
        panic!("protoVersion mismatch: expected {EXPECTED_PROTO}, got {proto:?}");
    }

    println!("SMOKE OK: thrum alive at {} (protoVersion={proto})", path.display());
    ExitCode::SUCCESS
}

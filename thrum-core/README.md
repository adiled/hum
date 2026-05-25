---
title: "thrum-core"
description: "wire-protocol primitives for thrum, the NDJSON socket between humd and bees"
---

# thrum-core

> _wire-protocol primitives for thrum, the NDJSON socket between humd and bees_

`thrum-core` is the Rust contract for **thrum**, the bidirectional
NDJSON socket protocol that bees (clients) and humds (daemons)
speak to each other. Importing this crate is how you build a Rust
bee that conforms to the protocol without copying constants
across repos.

## What's in it

- `Chi` — every wire-known message kind, kebab-case on serde. Bump
  `THRUM_VERSION` whenever you add a variant.
- `THRUM_VERSION` — the version string handed back in every `hello`.
- `WaneTracker` — Lamport-clock per sigil for drift detection +
  partition reconciliation.
- `Tone` — the loose JSON envelope every chi rides inside.
- `sigil(nest, sid)` — content-addressable handle for an inference
  context.

## Build a bee

```rust
use thrum_core::{Chi, THRUM_VERSION};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Connect to the local humd socket — humd binds it at
    // $XDG_RUNTIME_DIR/hum/thrum.sock by default.
    let sock = UnixStream::connect(humd_socket_path()).await?;
    let (rd, mut wr) = sock.into_split();
    let mut lines = BufReader::new(rd).lines();

    // Handshake. Optional fields (chi, propensity, source) feed the
    // on-mesh bee registry — see ensemble/README.md.
    let hello = json!({
        "chi": Chi::Hello,
        "rid": "hello-1",
        "from": "my-bee",
        "bee": "my-bee",
        "version": env!("CARGO_PKG_VERSION"),
        "protoVersion": THRUM_VERSION,
        "chis": ["hello", "prompt", "chunk", "finish"],
    });
    wr.write_all(format!("{hello}\n").as_bytes()).await?;

    // Read tones forever.
    while let Some(line) = lines.next_line().await? {
        let tone: serde_json::Value = serde_json::from_str(&line)?;
        match tone.get("chi").and_then(|v| v.as_str()) {
            Some("breath") => { /* daemon's handshake reply */ }
            Some("prompt") => { /* a turn arrived */ }
            _ => continue,
        }
    }
    Ok(())
}

fn humd_socket_path() -> std::path::PathBuf {
    let runtime = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| format!("/run/user/{}", unsafe { libc::geteuid() }));
    std::path::PathBuf::from(runtime).join("hum/thrum.sock")
}
```

## Using it from outside the repo

While the crate isn't on crates.io yet, git deps work today:

```toml
[dependencies]
thrum-core = { git = "https://github.com/adiled/hum.git" }
```

## Version policy

`THRUM_VERSION` is independent of crate version. Bumping the crate
package version is a Rust release event; bumping `THRUM_VERSION` is
a wire-protocol event. Bees warn on `THRUM_VERSION` mismatch.

| change | THRUM_VERSION bump |
|---|---|
| docstring tweaks, additive-optional fields | patch |
| new chi value, new required field with compat path | minor |
| removed chi, renamed chi, semantics changed | major |

## See also

- `ensemble/` — the mesh of humds. Bees discover each other
  through `bee_advertise` / `hive_discover` on the
  `hum/hives/announce` gossip topic.
- [`hives/`](../hives) — reference implementations. One
  canonical catalogue; don't enumerate them here.
- [adiled.github.io/hum/](https://adiled.github.io/hum/) — docs site.

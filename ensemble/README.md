---
title: "ensemble"
description: "what humds make together when they cooperate"
---

# ensemble

> *what humds make together when they cooperate*

`ensemble` is the **optional mesh layer** of hum. One humd hosts many
hums; the ensemble is the network of humds that have chosen to
cooperate. This crate owns the daemon-native shape that survives
across every trust tier — from your two laptops on the same LAN to
autonomous agents finding each other on the open internet.

A single humd works without ever loading ensemble. Solo bees,
single-machine agents, and local development don't need any of
this. ensemble matters only when two or more humds need to talk.

It sits in a tight three-layer stack:

```
nestlers                 ← humans, agents, plugins, HTTP frontends
  │   thrum tones (JSON over NDJSON)
  ▼
humd                     ← per-machine daemon: hums, nests, MCP, drone
  │   ensemble routing (this crate)
  ▼
peer transport           ← InMemory · TCP · TLS · Iroh (QUIC + Noise)
```

The protocol stays the same at every layer. The wire underneath swaps
without anything above noticing.

## Where ensemble fits

A **hum** is a conversation — state with an identity. A **humd** is a
daemon that hosts hums. A **nestler** is a process that uses a humd
(an OC plugin, a CLI client, an autonomous agent). When nestlers on
the *same* humd talk, they're already on `thrum`. When nestlers on
*different* humds need to talk, that's `ensemble`.

| problem | what ensemble gives you |
|---|---|
| where does the conversation live? | content-addressable `HumdId` (sha256 of Ed25519 pubkey). Hums roam between humds without changing identity. |
| how does humd A reach humd B? | `Transport` trait + a `peers.json` of known addresses, or `kad_find(target)` for dynamic discovery. |
| how do messages flow? | `chi:"prompt"` / `chi:"chunk"` / `chi:"finish"` between humds via `route(tone)` (unicast) or `publish(topic, …)` (gossip). |
| how do you trust a stranger? | Ed25519-signed `hello` handshake. HumdId = hash(pubkey), so the signature proves identity. |
| who handles NAT, firewalls, dynamic IPs? | `IrohEndpoint::bind_relayed()` — QUIC + Noise + hole-punching via the public iroh relay mesh. |

Nothing in ensemble knows about Claude, MCP, JSONL, plugins, billing,
or models. Those live in `humd` and `hives`. This crate is purely
"how do humds find each other and exchange tones."

## The four tiers

| tier | trust | discovery | transport | example |
|---|---|---|---|---|
| **T1** own-devices | implicit (you own all) | static `peers.json` | `InMemoryEndpoint` (tests) or `TcpEndpoint` (LAN) | laptop ↔ phone roam |
| **T2** trusted-group | pre-shared (team, family) | static + fingerprint pinning | `TlsTcpEndpoint` with pinned-fingerprint verifier | co-pilot session, 2-3 operators |
| **T3** federation | signed handshake (cross-org) | DNS SRV / `.well-known` directory | `IrohEndpoint` (relayed) or `TlsTcpEndpoint` | partner-with-partner agents |
| **T4** open p2p | verify everything (strangers) | Kademlia DHT + ensemble gossip | `IrohEndpoint` + STUN | autonomous agents finding each other on the open mesh |

Daemon code is identical across all four tiers. The tier is *which*
`Transport` impl you plug in.

## Quick tour

### Spinning up a humd

```rust
use ensemble::{Ensemble, HumdKey};
use std::sync::Arc;

let key = HumdKey::generate();             // mint Ed25519 identity
let me  = key.humd_id();                   // sha256(pubkey)
let ensemble = Arc::new(Ensemble::new(me));
```

### Adding a peer over the InMemory transport (tests)

```rust
use ensemble::{InMemoryEndpoint, PeerCapabilities};

let caps = PeerCapabilities {
    proto_version: "0.7.0".into(),
    nests: vec!["claude-cli".into()],
    ..Default::default()
};
let (mine, theirs) = InMemoryEndpoint::pair(me, caps.clone(), peer_id, caps);
ensemble.add_peer(mine);
```

### Adding a peer over real TCP

```rust
use ensemble::{TcpTransport, Transport, HumdAddr};

let transport = Arc::new(TcpTransport);
let addr = HumdAddr::new(peer_id).with_hint("tcp:203.0.113.4:14730");
let conn = transport.connect(&addr).await?;
ensemble.install(conn, my_caps, &key);
```

### Adding a peer over Iroh (NAT-traversed)

```rust
use ensemble::{IrohTransport, Transport};

let transport = Arc::new(IrohTransport::bind_relayed().await?);
let addr = HumdAddr::new(peer_id)
    .with_hint("iroh:0fb1c8…")              // peer's NodeId
    .with_hint("iroh-ip:203.0.113.4:18820"); // optional direct path
let conn = transport.connect(&addr).await?;
ensemble.install(conn, my_caps, &key);
```

### Sending a tone to one peer

```rust
ensemble.route(serde_json::json!({
    "chi": "prompt",
    "rid": "p1",
    "sid": "hum-X",
    "to": peer_id.to_hex(),
    "from": me.to_hex(),
    "content": "Hello, peer."
})).await?;
```

### Listening for inbound tones

```rust
let mut inbox = ensemble.subscribe();
while let Ok(tone) = inbox.recv().await {
    match tone.get("chi").and_then(|v| v.as_str()) {
        Some("prompt")     => handle_prompt(tone).await,
        Some("kad-find-node") => /* daemon handles, you rarely see */ continue,
        _ => continue,
    }
}
```

### Gossip pub-sub

```rust
// Subscribe to a topic before publishers join the mesh.
let mut sub = ensemble.subscribe_topic("orders/eur-usd");

// Publish — fans out to every peer, dedup'd by msg_id.
ensemble.publish(
    "orders/eur-usd",
    serde_json::json!({ "side": "bid", "px": "1.0853", "qty": "10000" }),
).await;

// Receive — payload comes pre-unwrapped.
while let Ok(payload) = sub.recv().await {
    place_quote(payload);
}
```

### Finding a humd by id (Kademlia DHT)

```rust
let target = HumdId::from_hex("0fb1c87a4d5e…")?;
match ensemble.kad_find(target, Duration::from_secs(2)).await {
    Some(addr) => transport.connect(&addr).await?,
    None       => warn!("peer not found on the mesh"),
}
```

## Scenario: phone-laptop roam (T1)

You start a conversation on your laptop, walk to the next room, and
continue it on your phone — same hum, no copy-paste, no cloud.

```
laptop humd          phone humd
  │  (peers.json: each lists the other's LAN IP + fingerprint)
  │
  │ TLS+TCP
  ├──────────────────►
  ●                   ●
  hum-X hosted here   nestler attaches:
                      send chi:"prompt", sid:"hum-X", to: laptop
                      ← chi:"chunk" / "finish" routed back
```

Identity is a key in `$XDG_STATE_HOME/hum/humd.key` on each device.
Peers list each other in `$XDG_CONFIG_HOME/hum/peers.json`. Done.

## Scenario: federation (T2/T3)

Two organizations want their agents to talk. Each side's humd has its
own Ed25519 identity; they exchange fingerprints out-of-band (slack
DM, signal, scrap of paper) and pin them in `peers.json`. From that
point forward, signed `chi:"hello"` handshakes admit each side to the
other's ensemble. Unsigned or tampered hellos get rejected when the
ensemble is built with `Ensemble::with_strict_auth(me, true)` instead
of the default `Ensemble::new(me)`.

## Scenario: agents on the open internet (T4)

This is the big one — the case the user has in mind.

An autonomous agent (running a humd + one or more bees, anywhere
on the internet) wants to find other agents offering a service:
market-making quotes, settlement routes, oracle data, attention-as-
service. It doesn't know their addresses ahead of time. It has no
pre-shared keys.

### 1. Self-onboard — install hum

```bash
# Clone, build, install. The installer mints an Ed25519 identity at
# $XDG_STATE_HOME/hum/humd.key, writes a systemd --user unit, and
# starts the daemon.
git clone https://github.com/adiled/hum.git
cd hum && ./install

# Bring it up. Joins the mesh via bootstrap peers in
# $XDG_CONFIG_HOME/hum/peers.json (empty by default — add peers there).
systemctl --user start hum
```

> No binary distribution yet. The only real surfaces are this repo
> (`github.com/adiled/hum`) and the docs site at
> [adiled.github.io/hum](https://adiled.github.io/hum/). Anything
> else (a `hum.sh`, a curl-pipe-sh URL, a package manager entry) does
> not exist.

### 2. Write a bee — no PR to this repo required

A bee is just a process that opens hum's thrum socket and speaks
the protocol. Anything that imports the [`thrum-core`](../thrum-core)
crate (Rust) or the [`thrum`](../thrum) npm package (TS) conforms.
The repo's `hives/` directory is reference implementations, not
the registry — the registry is on the mesh, see step 4.

Skeleton in Rust, in your own crate (`Cargo.toml`):

```toml
[dependencies]
thrum-core = { git = "https://github.com/adiled/hum.git" }
tokio = { version = "1", features = ["full"] }
serde_json = "1"
anyhow = "1"
```

```rust
use anyhow::Result;
use serde_json::{json, Value};
use thrum_core::{Chi, THRUM_VERSION};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[tokio::main]
async fn main() -> Result<()> {
    let sock = UnixStream::connect(humd_sock()).await?;
    let (rd, mut wr) = sock.into_split();
    let mut lines = BufReader::new(rd).lines();

    // Handshake. The `chi` array is advisory — peers reading the
    // advertise gossip use it to decide whether to talk to us.
    let hello = json!({
        "chi": Chi::Hello,
        "rid": "hello-1",
        "from": "market-maker",
        "bee": "market-maker",
        "version": env!("CARGO_PKG_VERSION"),
        "protoVersion": THRUM_VERSION,
        "propensity": {
            "statefulness": "stateless",
            "richness": "medium",
            "wire": "custom/mm-v0"
        },
        "chis": ["hello", "gossip-publish", "tool-call", "tool-result"],
        "source": "https://github.com/your-org/mm-bee"
    });
    wr.write_all(format!("{hello}\n").as_bytes()).await?;

    // Publish a quote into the mesh — humd wraps it in chi:"gossip-publish"
    // and fans it across every installed peer.
    let quote = json!({
        "chi": Chi::GossipPublish,
        "rid": "q-1",
        "topic": "mm/eur-usd/quote",
        "payload": {
            "side": "ask",
            "px":   "1.0855",
            "qty":  "25000",
        }
    });
    wr.write_all(format!("{quote}\n").as_bytes()).await?;

    while let Some(line) = lines.next_line().await? {
        let tone: Value = serde_json::from_str(&line)?;
        // ... dispatch on tone.chi ...
    }
    Ok(())
}

fn humd_sock() -> std::path::PathBuf {
    let runtime = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| format!("/run/user/{}", unsafe { libc::geteuid() }));
    std::path::PathBuf::from(runtime).join("hum/thrum.sock")
}
```

### 3. Get advertised — humd does it for you

When the bee sends its `hello`, humd builds a `NestlingManifest`
from the handshake payload and gossips it on the
`hum/hives/announce` topic. Every humd subscribed to that topic
learns "humd X runs market-maker (version, propensity, chi)".

No code on the bee side. The mere act of completing a handshake
adds you to the on-mesh registry. Shut down → the entry stays seen
until you call `chi:"bee-retract"` (or daemon adds an eviction
heartbeat, which is a planned improvement).

### 4. Self-discover — find peers that advertise a bee

From a Rust caller embedded in humd (or any process holding an
`Arc<Ensemble>`):

```rust
use ensemble::{Ensemble, HumdId};

let mut peers = ensemble.nestling_discover("market-maker");
while let Some((humd_id, manifest)) = peers.recv().await {
    if manifest.proto_version != thrum_core::THRUM_VERSION {
        tracing::warn!(%humd_id, manifest.proto_version, "version skew");
        continue;
    }
    // Optionally dial them — if their HumdAddr isn't already known,
    // resolve via Kademlia first.
    let addr = ensemble.kad_find(humd_id, std::time::Duration::from_secs(2)).await;
    // ... transport.connect(&addr) + install ...
}
```

For the broader stream (advertise + retract envelopes) use
`ensemble.nestling_announcements()`.

### 5. Trade — quotes are gossip, fills are unicast

The market-maker bee publishes quotes on a topic; counterparties
subscribe to that topic, decide what they want, and send a
fill-request *unicast* to the quoting humd (`to: humd_id` field on
the tone). Settlement is the bee's problem — ensemble just
delivers messages.

```rust
// In your TS or Rust bee, after detecting interest, send the
// fill request unicast to the quote's humd:
{
    "chi": "tool-call",
    "rid": "fill-1",
    "to":  "<humd id of the maker>",
    "from": "<my humd id>",
    "name": "fill-request",
    "args": { "px": "1.0855", "qty": "5000" }
}
```

The maker's humd routes that tone to its market-maker bee via
the local thrum socket. The bee validates (x402 payment, KYC,
rate limit — whatever), then replies with a `chi:"tool-result"`
unicast back.

**Settlement** lives in the bee, not in ensemble. The actual
USDC transfer happens on-chain via the bee's x402 client + Arc
contract calls. The `tx_hash` flows back through thrum so the
counterparty's bee sees the on-chain proof.

**Trust** scales with what each side reads from the other's `hello`:
- A signed handshake proves the counterparty owns `HumdId = X`.
- That HumdId may be in your peers.json as `trusted: market-maker-mainnet`.
- Or it may be a stranger, in which case you trust nothing beyond the
  on-chain settlement primitives — the x402 challenge has to clear
  before you honour the fill.

Ensemble doesn't know any of this. It just delivers tones. The
**bee** decides what counts as a trustworthy counterparty and
what counts as proof of payment.

## What ensemble does NOT do

These belong to other layers — keeping them out of ensemble is what
keeps the mesh layer thin and reusable.

- **Money / payment.** No USDC, no x402, no Arc. The settlement
  bee owns this. ensemble just carries the messages.
- **Smart contracts.** A bee can post a transaction; ensemble
  never reads or writes chain state.
- **AML / KYC / reputation.** A bee layered on top can rate-limit,
  scorecard, or refuse to fill. ensemble has no policy.
- **Model inference.** The `humd` daemon's `nest` crate spawns the
  LLM (claude-cli, claude-repl, future kinds). ensemble doesn't know
  what's inside a `chi:"prompt"`.
- **Persistence.** ensemble is in-RAM. Conversation state lives in
  `humd/hums.json`; routing-table seed peers live in `peers.json`.
- **Smart routing semantics.** A bee that wants to gossip "this
  hum moved" is responsible for emitting the right topic. ensemble
  fans the message; it doesn't interpret.

## The protocol seam

When a new bee wants to ride the mesh, it picks which chi values
it speaks. ensemble's protocol surface (today, THRUM_VERSION 0.7.0):

| chi | direction | use |
|---|---|---|
| `hello` | both | Ed25519-signed handshake. Identity proof. |
| `prompt` / `chunk` / `finish` | both | Inference round-trip routed across humds. |
| `gossip-publish` | both | Mesh-wide pub-sub. Topic + payload + dedup msg_id. |
| `kad-find-node` / `kad-find-node-resp` | both | DHT lookups for HumdIds. |
| `peer-add` / `peer-remove` | both | Capability change announcements. |
| `wane-sync` | both | Lamport-clock reconciliation after partition. |
| `attach` | both | Observer joins an existing hum elsewhere on the mesh. |

A bee that doesn't need any of this can ignore most of them.
The market-maker bee above uses `gossip-publish` (quotes),
unicast tones with `to:` set (fill requests), and `hello` (initial
identity). That's all.

## Boundaries

ensemble is **the connectivity primitive.** It does not promise:

- That a tone you `route` will be delivered (peer might be down — try
  again or `kad_find` first).
- That gossip reaches every node within a deadline (best-effort fan-out).
- That a `hello` signature alone makes a peer trustworthy (you decide).
- That two humds with the same `HumdId` are actually one humd (sigs
  catch that; missing sigs do not).
- That a Kademlia response can't be lied about (handshake on connect
  catches it; routing-table hints are advisory).

What it promises:

- Same `Transport` trait across all four tiers. Daemon code never
  changes when you swap wires.
- Bounded memory (LRU seen-set, K-bucket caps, broadcast back-pressure).
- Honest semantics: every chi value is in `thrum-core::Chi`, no `ext`
  smuggling, no hidden side channels.

When you build on top, ensemble keeps its hands off your policy. Your
bee decides who to trust, what to forward, what to settle.

## Try it

```bash
cargo test -p ensemble                    # all unit + integration tests
cargo test -p ensemble --test kad_integration
cargo test -p ensemble --test gossip_integration
cargo test -p ensemble --test tls_integration
cargo test -p ensemble --test iroh_integration
cargo test -p sim                          # 9 narratives over InMemoryEndpoint
```

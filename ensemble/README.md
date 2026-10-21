---
title: "ensemble"
description: "what humds make together when they cooperate"
---

# ensemble

> *what humds make together when they cooperate*

`ensemble` is the **optional mesh layer** of hum. One humd hosts many
hums; the ensemble is the network of humds that have chosen to
cooperate. This crate owns the daemon-native shape that survives
across every trust tier, from your two laptops on the same LAN to
autonomous agents finding each other on the open internet.

A single humd works without ever loading ensemble. Solo bees,
single-machine agents, and local development don't need any of
this. ensemble matters only when two or more humds need to talk.

It sits in a tight three-layer stack:

```text
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

The crate can carry more than the daemon currently drives. Two
sections keep that difference visible: [what the daemon wires
up](#what-the-daemon-wires-up) and [not here yet](#not-here-yet).

## Where ensemble fits

A **hum** is a conversation: state with an identity. A **humd** is a
daemon that hosts hums. A **nestler** is a process that uses a humd
(an OC plugin, a CLI client, an autonomous agent). When nestlers on
the *same* humd talk, they're already on `thrum`. When nestlers on
*different* humds need to talk, that's `ensemble`.

| problem | what ensemble gives you |
|---|---|
| where does the conversation live? | content-addressable `Hid` (sha256 of Ed25519 pubkey). Hums roam between humds without changing identity. |
| how does humd A reach humd B? | `Transport` trait + a `peers.json` of known addresses, or `kad_find(target)` for discovery among peers already known. |
| how do messages flow? | `chi:"prompt"` / `chi:"chunk"` / `chi:"finish"` between humds via `route(tone)` (unicast) or `publish(topic, …)` (gossip). |
| how do you trust a stranger? | Ed25519-signed `hello` handshake. `Hid` = hash(pubkey), so the signature proves identity. |
| who handles NAT, firewalls, dynamic IPs? | `IrohEndpoint::bind_relayed()`, which is QUIC + Noise + hole-punching via the public iroh relay mesh. The daemon binds direct-only; the relay path lives in the crate and needs a network that can reach the relay servers. |

Nothing in ensemble knows about models, MCP, JSONL, plugins, or
billing. Those live in `humd` and `hives`. This crate is purely
"how do humds find each other and exchange tones."

## The four tiers

| tier | trust | discovery | transport | example |
|---|---|---|---|---|
| **T1** own-devices | implicit (you own all) | static `peers.json` | `InMemoryEndpoint` (tests) or `TcpEndpoint` (LAN) | laptop ↔ phone roam |
| **T2** trusted-group | pre-shared (team, family) | static + fingerprint pinning | `TlsTcpEndpoint` with pinned-fingerprint verifier | co-pilot session, 2-3 operators |
| **T3** federation | signed handshake (cross-org) | static `peers.json` | `IrohEndpoint` (direct) or `TlsTcpEndpoint` | partner-with-partner agents |
| **T4** open p2p | verify everything (strangers) | Kademlia DHT + ensemble gossip | `IrohEndpoint` + relay | autonomous agents finding each other on the open mesh |

Daemon code is identical across all four tiers. The tier is *which*
`Transport` impl you plug in. The transport column lists what the
crate implements; the daemon ships T1 and T3 over TCP and direct
iroh, and wires neither TLS nor the relay. See [not here
yet](#not-here-yet).

## Quick tour

Every Rust block below is compiled by `cargo test -p ensemble`, and
the ones that can run do. The README is included as doctests, so an
example that stops being true stops the build.

### Spinning up a humd

```rust
use ensemble::{Ensemble, HumdKey};

let key = HumdKey::generate();
let me = key.hid();
let ensemble = Ensemble::new(me);
```

### Adding a peer over the InMemory transport

```rust
use ensemble::{Ensemble, HumdKey, InMemoryEndpoint, PeerCapabilities};

#[tokio::main]
async fn main() {
    let me_key = HumdKey::generate();
    let peer_key = HumdKey::generate();
    let me = me_key.hid();
    let peer_id = peer_key.hid();

    let caps = PeerCapabilities {
        proto_version: "0.7.0".into(),
        nests: vec!["market-maker".into()],
        ..Default::default()
    };

    let (mine, theirs) = InMemoryEndpoint::pair(me, caps.clone(), peer_id, caps.clone());
    let here = Ensemble::new(me);
    let there = Ensemble::new(peer_id);
    here.install(mine, caps.clone(), &me_key);
    there.install(theirs, caps, &peer_key);
}
```

### Adding a peer over real TCP

```rust,no_run
use ensemble::{Hid, HumdAddr, TcpTransport, Transport};

# async fn run() -> anyhow::Result<()> {
let peer_id = Hid::random_humd();
let transport = TcpTransport::new();
let addr = HumdAddr::new(peer_id).with_hint("tcp:203.0.113.4:14730");
let conn = transport.connect(&addr).await?;
# Ok(())
# }
```

### Adding a peer over Iroh

```rust,no_run
use ensemble::{Hid, HumdAddr, IrohTransport, Transport};

# async fn run() -> anyhow::Result<()> {
let peer_id = Hid::random_humd();
let transport = IrohTransport::bind_relayed().await?;
let addr = HumdAddr::new(peer_id)
    .with_hint("iroh:0fb1c87a4d5e4e3a9c1b2d3e4f5061728394a5b6c7d8e9f0a1b2c3d4e5f60718")
    .with_hint("iroh-ip:203.0.113.4:18820");
let conn = transport.connect(&addr).await?;
# Ok(())
# }
```

### Sending a tone to one peer

```rust,no_run
use ensemble::{Ensemble, Hid};
use serde_json::json;

# async fn run(ensemble: &Ensemble, me: Hid, peer_id: Hid) -> anyhow::Result<()> {
ensemble.route(json!({
    "chi": "prompt",
    "rid": "p1",
    "sid": "hum-X",
    "to": peer_id.to_hex(),
    "from": me.to_hex(),
    "content": "Hello, peer."
})).await?;
# Ok(())
# }
```

### Listening for inbound tones

```rust,no_run
use ensemble::Ensemble;

# async fn run(ensemble: &Ensemble) {
let mut inbox = ensemble.subscribe();
while let Ok(tone) = inbox.recv().await {
    match tone.get("chi").and_then(|v| v.as_str()) {
        Some("prompt") => println!("prompt: {tone}"),
        Some("kad-find-node") => continue,
        _ => continue,
    }
}
# }
```

### Gossip pub-sub

```rust,no_run
use ensemble::Ensemble;
use serde_json::json;

# async fn run(ensemble: &Ensemble) {
let mut sub = ensemble.subscribe_topic("orders/eur-usd");

ensemble.publish(
    "orders/eur-usd",
    json!({ "side": "bid", "px": "1.0853", "qty": "10000" }),
).await;

while let Ok(payload) = sub.recv().await {
    println!("quote: {payload}");
}
# }
```

### Finding a humd by id (Kademlia)

```rust,no_run
use ensemble::{Ensemble, Hid};
use std::time::Duration;

# async fn run(ensemble: &Ensemble) {
let target = Hid::from_hex(
    "0fb1c87a4d5e4e3a9c1b2d3e4f5061728394a5b6c7d8e9f0a1b2c3d4e5f60718",
).expect("a humd id is 32 bytes of hex");

match ensemble.kad_find(target, Duration::from_secs(2)).await {
    Some(addr) => println!("found {addr:?}"),
    None => println!("not found among known peers"),
}
# }
```

## What the daemon wires up

The crate is bigger than the daemon. Today, `humd`:

- speaks **TCP** (`humd.tcpListen`) and **iroh** over a direct path
  (`humd.irohPort`, default `41641`);
- prefers **iroh** when dialing, and falls back to TCP only for peers
  iroh could not reach;
- discovers peers from `peers.json` plus the liveness supervisor's
  redial, and keeps one link per peer;
- **rejects unsigned hellos**: `Ensemble::new` is strict by default,
  and `with_strict_auth(me, false)` is the opt-out;
- publishes its **own** hive manifests on the `hum/hives/announce`
  topic, and re-advertises them when a peer joins, so a peer that
  arrives late still learns what the mesh offers;
- routes unicast tones and relays gossip between the peers it holds.

## Not here yet

These are written as if they work. They do not.

- **A bee cannot publish to the mesh.** The daemon's sink handles
  `hello`, `prompt`, `cancel`, `attach`, `peer-add`, `peer-remove`,
  `wane-sync` and the rest, but there is no `gossip-publish` arm,
  so a tone a bee writes to the local thrum socket never reaches a
  peer. The daemon publishes hive manifests; a bee's own topics do
  not travel.
- **There is no retraction.** `chi:"bee-retract"` does not exist, and
  the ensemble's `hive_retract` is called by nothing but a test. A
  manifest that is announced stays announced.
- **The DHT cannot bootstrap.** The Kademlia table is seeded from
  `peers.json` and from the persisted routing store. There is no DNS
  seeding, no bootstrap node, no contact exchange, so `kad_find` can
  only find peers the node already knows.
- **The relay path is unproven.** `bind_relayed()` exists; the n0
  relay servers were unreachable from the network this was developed
  on, and no traffic has crossed a relay.
- **TLS is a library capability the daemon never uses.**
  `TlsTcpEndpoint` and its pinned-fingerprint verifier are here and
  tested; `humd` wires only TCP and iroh.
- **The transport that survives is not the preferred one.** One link
  per peer means the last link admitted wins, and the two sides
  pick independently. With both TCP and iroh configured, the mesh
  can end up on the plaintext link even though the iroh dial won the
  race.

## Scenario: phone-laptop roam (T1)

You start a conversation on your laptop, walk to the next room, and
continue it on your phone: same hum, no copy-paste, no cloud.

```text
laptop humd          phone humd
  │  (peers.json: each lists the other's LAN address)
  │
  │ TCP or Iroh
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
other's ensemble. Unsigned or tampered hellos are rejected, because the
ensemble is strict by default.

## Scenario: agents on the open internet (T4)

This is the case worth building toward, and it is the one that is
least built. An autonomous agent wants to find other agents offering
a service without knowing their addresses and without pre-shared
keys. Three pieces have to exist for that, and two of them do not:

1. **Onboard**: install hum, and the daemon mints an Ed25519
   identity at `$XDG_STATE_HOME/hum/humd.key`, writes a
   `systemd --user` unit, and starts.

   ```bash
   git clone https://github.com/adiled/hum.git
   cd hum && ./install
   systemctl --user start humd
   ```

   > No binary distribution yet. The only real surfaces are this repo
   > (`github.com/adiled/hum`) and the docs site at
   > [adiled.github.io/hum](https://adiled.github.io/hum/). Anything
   > else (a `hum.sh`, a curl-pipe-sh URL, a package manager entry) does
   > not exist.

2. **Get advertised**: when a bee completes its handshake, the
   daemon builds a `HiveManifest` and gossips it on the
   `hum/hives/announce` topic. Every humd subscribed to that topic
   learns "humd X runs market-maker (version, propensity, chi)". The
   mere act of completing a handshake adds you to the on-mesh
   registry. There is no retraction, and no heartbeat that evicts a
   manifest whose bee has gone away.

3. **Get discovered**: a peer that wants a service subscribes to the
   announce topic, and for each manifest it does not recognise asks
   the DHT where that humd lives:

   ```rust,no_run
   use ensemble::{Ensemble, Hid};
   use std::time::Duration;

   # async fn run(ensemble: &Ensemble) {
   let mut seen = ensemble.hive_discover("market-maker");
   while let Some((humd_id, manifest)) = seen.recv().await {
       if manifest.proto_version != thrum_core::THRUM_VERSION {
           continue;
       }
       let addr = ensemble.kad_find(humd_id, Duration::from_secs(2)).await;
       println!("{humd_id:?} at {addr:?}");
   }
   # }
   ```

   For the broader stream (advertise and retract envelopes) use
   `ensemble.hive_announcements()`.

4. **Trade**: this is the part that does not work yet. A bee that
   wants to publish its own quotes to the mesh cannot: the daemon
   publishes hive manifests and nothing else, and there is no
   `gossip-publish` arm in the sink. Unicast works: a tone with
   `to:` set is routed to the peer that owns that humd, so
   request/response between two known peers is fine today. What is
   missing is a bee putting a topic on the mesh and its
   counterparties subscribing to it.

**Trust** scales with what each side reads from the other's `hello`:
a signed handshake proves the counterparty owns the `Hid` it claims,
and nothing more. Whether that counterparty is worth a trade is the
bee's call. Settlement, payment, rate limits, and reputation all
live above the mesh. ensemble carries tones; it does not interpret
them.

## What ensemble does NOT do

These belong to other layers. Keeping them out of ensemble is what
keeps the mesh layer thin and reusable.

- **Money / payment.** No USDC, no x402, no Arc. The settlement
  bee owns this. ensemble just carries the messages.
- **Smart contracts.** A bee can post a transaction; ensemble
  never reads or writes chain state.
- **AML / KYC / reputation.** A bee layered on top can rate-limit,
  scorecard, or refuse to fill. ensemble has no policy.
- **Model inference.** The nest (orchd and the hives) spawns the
  worker that answers a `chi:"prompt"`. ensemble doesn't know what
  is inside the prompt.
- **Persistence.** ensemble is in-RAM. The daemon keeps its own
  state under `$XDG_STATE_HOME/hum`; peer addresses live in
  `peers.json`.
- **Routing semantics.** A bee that wants to gossip "this hum moved"
  is responsible for emitting the right topic. ensemble fans the
  message; it doesn't interpret.

## The protocol surface

When a new bee wants to ride the mesh, it picks which chi values it
speaks. The surface today, THRUM_VERSION 0.7.0:

| chi | direction | use |
|---|---|---|
| `hello` | both | Ed25519-signed handshake. Identity proof. |
| `prompt` / `chunk` / `finish` | both | Inference round-trip routed across humds. |
| `gossip-publish` | both | Mesh-wide pub-sub. Topic + payload + dedup msg_id. |
| `kad-find-node` / `kad-find-node-resp` | both | DHT lookups for `Hid`s. |
| `peer-add` / `peer-remove` | both | A peer joined or left. The receiver re-advertises its manifests to a newcomer. |
| `wane-sync` | both | Lamport-clock reconciliation after partition. |
| `attach` / `detach` | both | Observer joins an existing hum. Handled by the daemon, not the mesh. |

A bee that doesn't need any of this can ignore most of them.

## Boundaries

ensemble is **the connectivity primitive.** It does not promise:

- That a tone you `route` will be delivered (the peer might be down; try
  again or `kad_find` first).
- That gossip reaches every node within a deadline (best-effort
  fan-out).
- That a `hello` signature alone makes a peer trustworthy (you decide).
- That two humds with the same `Hid` are actually one humd (sigs
  catch that; missing sigs do not).
- That a Kademlia response can't be lied about (handshake on connect
  catches it; routing-table hints are advisory).

What it promises:

- The same `Transport` trait for every transport in the crate, so
  daemon code never changes when you swap wires.
- Bounded memory (LRU seen-set, K-bucket caps, broadcast back-pressure).
- Honest semantics: every chi value is in `thrum-core::Chi`, no `ext`
  smuggling, no hidden side channels.

When you build on top, ensemble keeps its hands off your policy. Your
bee decides who to trust, what to forward, what to settle.

## Try it

```bash
cargo test -p ensemble
cargo test -p ensemble --test gossip_integration
cargo test -p ensemble --test hives_integration
cargo test -p ensemble --test iroh_integration
cargo test -p ensemble --test kad_integration
cargo test -p ensemble --test tcp_integration
cargo test -p ensemble --test tls_integration
cargo test -p sim
```

The README's own examples are part of that run: `ensemble/tests/readme.rs`
includes this file as doctests, so a snippet that stops compiling
fails the build.

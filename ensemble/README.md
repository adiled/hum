# ensemble

> *what humds make together when they cooperate*

`ensemble` is the mesh layer of hum. One humd hosts many hums; the
ensemble is the network of humds. This crate owns the daemon-native
shape that survives across every trust tier — from your two laptops
on the same LAN to autonomous agents finding each other on the
open internet.

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
or models. Those live in `humd` and `nestlings`. This crate is purely
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
ensemble.install(conn, my_caps);
```

### Adding a peer over Iroh (NAT-traversed)

```rust
use ensemble::{IrohTransport, Transport};

let transport = Arc::new(IrohTransport::bind_relayed().await?);
let addr = HumdAddr::new(peer_id)
    .with_hint("iroh:0fb1c8…")              // peer's NodeId
    .with_hint("iroh-ip:203.0.113.4:18820"); // optional direct path
let conn = transport.connect(&addr).await?;
ensemble.install(conn, my_caps);
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
other's ensemble. Unsigned or tampered hellos get rejected by
`Ensemble::with_strict_auth(true)`.

## Scenario: agents on the open internet (T4)

This is the big one — the case the user has in mind.

An autonomous agent (running a humd + one or more nestlings, anywhere
on the internet) wants to find other agents offering a service:
market-making quotes, settlement routes, oracle data, attention-as-
service. It doesn't know their addresses ahead of time. It has no
pre-shared keys.

**Self-onboard:**

```bash
# One-line install (assumes hum publishes a binary distribution).
curl -fsSL https://hum.sh/install | sh

# Daemon auto-mints an identity, picks a port, joins the mesh via
# bootstrap peers shipped in the default config.
systemctl --user start hum
```

**Self-discover:**

The new humd binds an `IrohEndpoint::bind_relayed()`, performs
handshakes against bootstrap peers, then runs `kad_find` for the
specific HumdIds it needs (or queries topics it cares about via
gossip).

```rust
// At boot — find peers advertising the nest kind we need.
ensemble.publish("hum/discover", json!({
    "wants": ["market-maker", "x402-settle"],
    "from":  me.to_hex(),
})).await;
```

Peers running matching nestlings respond on the same topic with their
HumdId + capabilities. The new humd installs connections to a few of
them and starts trading.

**Cooperate via nestlings:**

The agent's market-making logic lives in a `market-maker` nestling
(a process that connects to its local humd's thrum socket). Quotes
are gossip; fills are unicast.

```rust
// Inside the market-maker nestling — pseudocode using the TS
// nestlings/* shape, but it works identically in Rust.

// Publish a quote into the mesh.
thrum.send({
    chi: "gossip-publish",
    rid: rid(),
    topic: "mm/eur-usd/quote",
    payload: {
        humd_id: me.to_hex(),
        side: "ask",
        px:   "1.0855",
        qty:  "25000",
        expires_at: now() + 30_000,
    },
});

// Listen for fills addressed at us.
for await (const tone of thrum.subscribe("inbound")) {
    if (tone.chi !== "fill-request") continue;
    if (tone.to  !== me.to_hex()) continue;

    // Validate the counterparty's x402 payment, then settle on Arc.
    const ok = await x402_settle(tone.from, tone.payload);
    thrum.send({
        chi: "fill-confirm",
        rid: rid(),
        sid: tone.sid,
        to:  tone.from,
        from: me.to_hex(),
        status: ok ? "settled" : "rejected",
        tx_hash: ok ? tx.hash : null,
    });
}
```

**Settlement** lives in the nestling, not in ensemble. ensemble is
the transport for the conversation between agents. The actual USDC
transfer happens on-chain via the nestling's x402 client + Arc
contract calls. The `tx_hash` flows back through thrum so the
counterparty's nestling sees the on-chain proof.

**Trust** scales with what each side reads from the other's `hello`:
- A signed handshake proves the counterparty owns `HumdId = X`.
- That HumdId may be in your address book as `trusted: market-maker-mainnet`.
- Or it may be a stranger, in which case you trust nothing beyond the
  on-chain settlement primitives — the x402 challenge has to clear
  before you honour the fill.

Ensemble doesn't know any of this. It just delivers tones. The
**nestling** decides what counts as a trustworthy counterparty and
what counts as proof of payment.

## What ensemble does NOT do

These belong to other layers — keeping them out of ensemble is what
keeps the mesh layer thin and reusable.

- **Money / payment.** No USDC, no x402, no Arc. The settlement
  nestling owns this. ensemble just carries the messages.
- **Smart contracts.** A nestling can post a transaction; ensemble
  never reads or writes chain state.
- **AML / KYC / reputation.** A nestling layered on top can rate-limit,
  scorecard, or refuse to fill. ensemble has no policy.
- **Model inference.** The `humd` daemon's `nest` crate spawns Claude
  (or whatever). ensemble doesn't know what's inside a `chi:"prompt"`.
- **Persistence.** ensemble is in-RAM. Conversation state lives in
  `humd/hums.json`; routing-table seed peers live in `peers.json`.
- **Smart routing semantics.** A nestling that wants to gossip "this
  hum moved" is responsible for emitting the right topic. ensemble
  fans the message; it doesn't interpret.

## The protocol seam

When a new nestling wants to ride the mesh, it picks which chi values
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

A nestling that doesn't need any of this can ignore most of them.
The market-maker nestling above uses `gossip-publish` (quotes),
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
nestling decides who to trust, what to forward, what to settle.

## Try it

```bash
cargo test -p ensemble                    # all unit + integration tests
cargo test -p ensemble --test kad_integration
cargo test -p ensemble --test gossip_integration
cargo test -p ensemble --test tls_integration
cargo test -p ensemble --test iroh_integration
cargo test -p sim                          # 9 narratives over InMemoryEndpoint
```

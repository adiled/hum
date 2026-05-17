---
title: "bp7-nestling (Rust)"
description: "RFC 9171 Bundle Protocol v7 — interplanetary store-and-forward bridge to hum"
---

# bp7-nestling

> _the interplanetary nestling — accept BP7 bundles from spacecraft and other DTN nodes; reply via the same store-and-forward path_

Hum, but over **DTN**. The Delay/Disruption-Tolerant Networking
protocol used by NASA's ION, ESA's tests on the ISS, and the planned
lunar Gateway. RFC 9171. CBOR on the wire. Designed for the actual
solar system: one-way delays measured in minutes (Mars) to hours
(Voyager).

A bundle addressed to `dtn://hum.local/inference` arrives over UDP
(or any other convergence layer your DTN router provides). We decode,
extract the payload, hand it to humd as `chi:"prompt"`, collect the
chunks, wrap the answer in a reply bundle, and send it back to the
bundle's source EID. Real protocol; works today against a real DTN
router on your laptop.

## Propensity

| statefulness | richness | wire shape | hides |
|---|---|---|---|
| stateless per-call | lean | RFC 9171 BP7 over UDPCL | tools, system, perf, drone, breath, permission-ask |

## Wire

```
DTN peer ──BP7 bundle (CBOR over UDP:4556)──► bp7-nestling ──chi:"prompt"──► humd
                                                                                │
DTN peer ◄──BP7 bundle (CBOR over UDP)──── bp7-nestling ◄──chi:"finish"────────┘
```

Payload may be:

- **Plain UTF-8 text** — treated as the prompt body
- **JSON** — `{ "text": "...", "modelId": "claude-sonnet-4", "system": "..." }`

The bundle's source EID is reused as the reply destination. Reply
payload is the collected `chunk` text concatenated into a single
UTF-8 body. No SSE re-framing — DTN replies are *messages*, not
streams.

## Configure

| env | default | what |
|---|---|---|
| `BP7_LISTEN` | `0.0.0.0:4556` | UDP convergence-layer address |
| `BP7_NODE_EID` | `dtn://hum.local/inference` | this node's endpoint id (URI). Bundles addressed elsewhere are dropped |
| `BP7_MODEL` | `claude-sonnet-4` | model humd spawns when a payload doesn't specify one |
| `HUM_THRUM_SOCK` | `$XDG_RUNTIME_DIR/hum/thrum.sock` | humd's NDJSON socket |

## Run

```bash
cargo run -p bp7-nestling
```

Hit it from another machine running [NASA's ION](https://sourceforge.net/projects/ion-dtn/),
[µPCN](https://upcn.eu/), or [hDTN](https://github.com/nasa/HDTN):

```bash
# example with bp7-rs's own bin from your laptop
bp7-rs --src dtn://you/ \
  --dst dtn://hum.local/inference \
  --listen 0.0.0.0:4557 \
  --send '{"text":"What is the speed of light in glass?"}'
```

The reply bundle lands on your listening port whenever round-trip
latency permits. On a LAN: ~milliseconds. Through one TDRS relay
hop: ~600ms. Real Mars round-trip: 5–24 minutes.

## What this actually unlocks

- **A hum node addressable from any DTN-speaking peer in the solar system.** A spacecraft on Mars (or its simulation in your lab) can prompt your humd by store-and-forwarding a bundle that includes your EID. The reply rides the same path home.
- **Hum-the-protocol over physically partitioned links.** thrum was designed assuming local sockets are reliable; DTN was designed assuming *no* path exists at any given moment. The two layered together is a genuine partition-tolerant agent system.
- **Custody transfer**. A real DTN router holds your bundle, with custody, until the next hop confirms receipt. If a Mars probe drops the link mid-payload, the upstream router resends on the next contact window. The bp7-nestling itself doesn't implement custody (we're a leaf node); the surrounding DTN router does.

## What this nestling does NOT do

- **Route**. Bundles addressed to other EIDs get dropped, not forwarded. Run a real DTN router (ION / µPCN / hDTN) alongside this nestling and point its routing table at our UDP port if you want routing.
- **Authenticate**. BP7 has security extensions (BPSec); this v1 doesn't use them. Anyone reachable on UDP:4556 can prompt your humd.
- **Custody transfer**. We acknowledge a bundle by replying — that's the only signal upstream gets. A real DTN deployment layers custody on top.
- **Stream**. DTN is store-and-forward, not streaming. Your `chi:"chunk"` tones get collected into one reply bundle. If you want progress-via-bundle, send multiple intermediate bundles — that's a future extension.

## Why this is the right kind of unhinged

Hum was designed for local sockets and millisecond peer-to-peer. DTN
was designed for one-way delays of minutes to hours, intermittent
connectivity, and custody-transfer at every hop. Bridging the two
exposes every assumption hum makes about timing. The drone's `lost`
threshold fires after 30s; a Mars round trip is 600× that. The wane
counter ticks for a wall-clock minute and finds nothing. Your humd
discovers what it actually feels like to be a peer 90M km away.

The thought experiment: aim a humd at an actual Mars rover ground
station and see what falls off. (Don't — DSN time is contested.)

## See also

- [RFC 9171 — Bundle Protocol v7](https://datatracker.ietf.org/doc/rfc9171/)
- [NASA ION](https://sourceforge.net/projects/ion-dtn/) — the reference DTN router
- [bp7-rs](https://github.com/dtn7/bp7-rs) — the Rust crate this nestling uses
- [Vint Cerf, "An Internet for Mars"](https://www.youtube.com/watch?v=hKt-jK19vHU) — the architecture talk that started this
- [WIRE.md](../../thrum/WIRE.md) — the underlying thrum protocol

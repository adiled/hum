---
title: "bp7-forager (Rust)"
description: "RFC 9171 Bundle Protocol v7: hum as a DTN endpoint, and space's way into the ensemble"
---

# bp7-forager

> _the interplanetary bee: take a bundle from any DTN node, hold it until hum can answer, and send the answer home the way bundles go_

Hum, but over **DTN**: the Delay/Disruption-Tolerant Networking of NASA's
ION and HDTN, ESA's tests on the ISS, and the planned lunar Gateway.
RFC 9171, CBOR on the wire, built for links where one-way delay is
minutes (Mars) to hours (Voyager) and no end-to-end path exists at any
given moment.

A bundle addressed to `dtn://hum.local/inference` arrives over UDP
(RFC 7122) from a DTN router or node. The forager holds it on disk, asks
humd for a bloom, and sends the bloom's text back to the bundle's source
endpoint as a bundle of its own. The bloom need not grow here: humd hands
it to whichever nurse grows the model, on this humd or on a trusted peer
across the ensemble, so a node in space reaches every nurse the
ensemble has.

## Propensity

| statefulness | richness | wire shape | hides |
|---|---|---|---|
| holds each ask until it is answered or expires | lean | RFC 9171 BP7 over UDPCL | tools, perf, breath |

## Wire

```
DTN node ─bundle─► router ─UDP:4556─► bp7-forager ─► spool ─chi:"prompt"─► humd ─► nurse, here or across the ensemble
                                           │                                │
DTN node ◄─bundle─ router ◄─BP7_ROUTE─────┘◄──────────── chi:"wilt" ────────┘
```

**An ask** is a bundle whose payload is either plain UTF-8 text, taken
as the prompt, or JSON: `{"text": "...", "modelId": "claude-sonnet-4",
"system": "..."}`.

**The answer** is one bundle to the ask's source endpoint, or several
fragments of one when it is longer than `BP7_MTU`. Its payload is the
bloom's text, or what broke when the bloom broke off. Every answer
carries block 193 (RFC 9171 §4.3.2 leaves 192 to 255 for private use),
whose data is `{"hum": "reply", "to": "<the ask's bundle id>", "ok":
true|false}`: a DTN node that knows hum can match an answer to its ask,
and one that does not can ignore the block. Every block has a CRC-32
(§4.3.1). The answer lives as long as the ask was given to live, asks
for no status reports, and carries a hop limit of 32.

## What it holds to

- **Store and forward.** An ask is on disk before anything is done with
  it, and stays until its answer has gone home. A restart of the forager
  or of humd loses nothing; the bloom is grown again.
- **Heard once.** DTN delivers at least once. A bundle's id is its source
  and creation timestamp, the same in every copy and every fragment, and
  a copy of an ask held or answered is dropped until the ask expires.
- **Lifetime.** A bundle past its lifetime is deleted (§5.5), and so is
  an ask whose lifetime runs out while it waits. A node with no clock
  sends creation time 0 and a bundle age block (§4.4.2), and its asks
  expire by their age. The ask's expiry rides on the prompt as `dusk`.
- **Fragments.** An ask that arrives in pieces, in any order, with
  overlaps and repeats, is grown once from the whole (§5.9). An answer
  too long for one datagram is fragmented at the bundle layer, block 193
  in every fragment (§5.8), so no datagram needs IP fragmentation
  (RFC 7122 §3.2).
- **What is not an ask.** A bundle that fails its CRC or RFC 9171's
  checks, is for another endpoint, is an administrative record, is
  hum's own answer, or names no source (`dtn:none`) is never grown, so
  no two hums, and no status report, can keep a loop of blooms going.
- **No nurse yet.** An ask for a model no nurse grows waits, retried
  with backoff, until one comes or the ask expires. So does one whose
  humd is away.
- **Bounded.** At most `BP7_BLOOMS` blooms at once, and at most
  `BP7_HOLD` asks held; past that a bundle is dropped, as a node with
  depleted storage does. An ask is at most 4 MiB, however many pieces
  it arrives in.

## Configure

| env | default | what |
|---|---|---|
| `BP7_LISTEN` | `0.0.0.0:4556` | the UDP convergence-layer address (IANA's 4556) |
| `BP7_NODE_EID` | `dtn://hum.local/inference` | this endpoint; a bundle for any other is not ours |
| `BP7_LABEL` | `main` | the bee's label; give each forager on one humd its own |
| `BP7_MODEL` | `claude-sonnet-4` | the model when an ask names none |
| `BP7_ROUTE` | the address the bundle came from | where every answer goes: the router's UDPCL address, `host:port` |
| `BP7_ALLOW` | anyone | the IPs a bundle is taken from, comma-separated (RFC 7122 §5) |
| `BP7_BLOOMS` | `4` | blooms grown at once |
| `BP7_MTU` | `1232` | the largest datagram an answer goes in; 1232 fits any IPv6 path |
| `BP7_SPOOL` | `$XDG_STATE_HOME/hum/hives/bp7/spool` | where asks are held |
| `BP7_HOLD` | `10000` | asks held at once |
| `BP7_SWEEP_MS` | `60000` | how often what has expired is forgotten |

Behind a router, set `BP7_ROUTE`: a router sends from a port of its own
choosing, not the one it listens on, so the address a bundle came from is
rarely where an answer can go. Without a router, a node that sends from
the socket it listens on gets its answer there.

## Run

```bash
hum hive bp7 install                               # build it, enter it, keep it running
BP7_ROUTE=router.local:4556 hum hive bp7 install   # behind a router
```

The forager is a forager bee of the hive `bp7`; its hive must be
installed on the humd it lands on. For the ensemble to grow its asks on
another humd, that humd must be a peer, its nurse's hive installed here,
and this humd must trust remote nurses (`trustRemoteNurses` in
`hum.json`, or `HUM_TRUST_REMOTE_NURSES=1`).

## Test

```bash
cd hives/bp7 && cargo test
```

The tests are the forager as a DTN node meets it, against a real humd:
`tests/dtn.rs` covers duplicates, bursts, expiry, fragments both ways,
corruption, status reports, loops, routing and restarts; `tests/outlive.rs`
covers humd going away; `tests/ensemble.rs` has an ask from space grown by
a nurse on another humd, over TCP. `tests/common` holds a minimal DTN node,
which is also the shortest example of speaking to the forager.

## What is not yet

- **An ask whose grower leaves the ensemble waits out its lifetime.**
  When humd hands a bloom to a peer's nurse and that peer goes away
  mid-bloom, humd removes the peer but never breaks off the blooms it
  was growing, so the forager hears neither a wilt nor an error and
  cannot grow the ask again until the bundle expires, which may be days
  for an ask from deep space. humd also keeps which peer grew each bloom
  after it wilts. The fix belongs in humd: on losing a peer, send each
  bloom it was growing an `error` a forager can retry on, and forget a
  bloom's grower when it wilts. `tests/grower_gone.rs` is the test for
  it, ignored until then.
- **No BPSec.** Anyone who can reach the UDP port can prompt this humd;
  keep it behind a router, and set `BP7_ALLOW`. `bp7` 0.12 signs and
  verifies BIB-HMAC-SHA2 (RFC 9172, 9173), so authenticated asks are the
  next step.
- **No custody, no status reports.** The forager is a leaf: it sends no
  reception or delivery reports, even when an ask asks for them. The
  router around it does custody.
- **UDP only.** TCPCL and LTP are the router's to speak.
- **One answer per ask.** DTN answers are messages, not streams; the
  unfurls are gathered into one answer.
- **humd keeps every sid a connection opens until the connection
  closes.** The forager holds one connection for its life, so humd's
  memory grows a little with every bloom it grows.

## See also

- [RFC 9171](https://datatracker.ietf.org/doc/rfc9171/): Bundle Protocol v7
- [RFC 7122](https://datatracker.ietf.org/doc/rfc7122/): datagram convergence layers
- [RFC 9172](https://datatracker.ietf.org/doc/rfc9172/): BPSec
- [bp7-rs](https://github.com/dtn7/bp7-rs): the Rust crate this bee uses
- [NASA ION](https://sourceforge.net/projects/ion-dtn/) and [HDTN](https://github.com/nasa/HDTN): DTN routers
- [THRUM.md](../../THRUM.md): the protocol on humd's side

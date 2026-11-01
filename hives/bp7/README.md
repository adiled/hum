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

A bundle addressed to this endpoint (`dtn://hum.local/inference` unless
`BP7_NODE_EID` says otherwise) arrives over UDP (RFC 7122) from a DTN
router or node. The forager holds it on disk, asks humd for a bloom, and
sends the bloom's text back to the bundle's source endpoint as a bundle
of its own. The bloom need not grow here: humd hands
it to whichever nurse grows the model, on this humd or on a trusted peer
across the ensemble, so a node in space reaches every nurse the
ensemble has.

## Propensity

| statefulness | richness | wire shape | hides |
|---|---|---|---|
| `stateless` | `lean` | `bp7/dtn-udpcl` | tendrils, every petal but text, the unfurls as they grow |

Each ask, and each try at it, is a bloom on a sid of its own, and the
ask is held on disk until it is answered or expires. The forager sounds
`prompt`, `cancel` when an ask expires mid-bloom, and `cleanup` for a
sid once its bloom is over, and hears `unfurl`, `wilt` and `error`; its
hello's `chis` names these, so humd sends it nothing else of a bloom.

## Wire

```
DTN node ─bundle─► router ─UDP:4556─► bp7-forager ─► spool ─chi:"prompt"─► humd ─► nurse, here or across the ensemble
                                           │                                │
DTN node ◄─bundle─ router ◄─BP7_ROUTE─────┘◄──────────── chi:"wilt" ────────┘
```

**An ask** is a bundle whose payload is either a JSON object with a
`text` string, `{"text": "...", "modelId": "claude-sonnet-4-6", "system":
"..."}`, where `modelId` and `system` may be left out, or anything else,
taken whole as the prompt's text (as UTF-8). An ask that names no
`modelId` takes `BP7_MODEL`, and with neither its prompt names no model,
so humd hands it to any nurse with room.

**The answer** is one bundle to the ask's source endpoint, or several
fragments of one when it is longer than `BP7_MTU`. Its payload is the
bloom's text, or what broke when the bloom broke off. Every answer
carries block 193 (RFC 9171 §4.3.2 leaves 192 to 255 for private use),
whose data is `{"hum": "reply", "to": "<the ask's id>", "ok":
true|false}`, the id being `<source>-<creation time>-<sequence number>`
as the ask carried them: a DTN node that knows hum can match an answer
to its ask, and one that does not can ignore the block. Every block has
a CRC-32 (§4.3.1). The answer lives as long as the ask was given to
live, asks for no status reports, and carries a hop limit of 32.

## What it holds to

- **Store and forward.** An ask is on disk before anything is done with
  it, and stays until its answer has gone home. A restart of the forager
  or of humd loses nothing: an ask mid-bloom is grown again, and an
  answer already grown goes home as it is.
- **Heard once.** DTN delivers at least once. A bundle's id is its source
  and creation timestamp, the same in every copy and every fragment, and
  a copy of an ask held or answered is dropped until the ask expires.
- **Lifetime.** A bundle past its lifetime is deleted (§5.5), and so is
  an ask whose lifetime runs out while it waits. A node with no clock
  sends creation time 0 and a bundle age block (§4.4.2), and its asks
  expire by their age. The ask's expiry rides on the prompt as `dusk`.
- **Cleaned up.** No bloom grows on a try's sid again, so once the try's
  bloom is over, wilted, broken off, cancelled at the ask's expiry, or
  refused for want of a nurse, the forager sends `cleanup` for its sid.
  A try humd took with it when it went away needs none, and one a
  stopped forager left growing is cleaned up once the forager is back.
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
  with backoff at most 30 s apart, until one comes or the ask expires.
  So does one whose humd is away.
- **Bounded.** At most `BP7_BLOOMS` blooms at once, and at most
  `BP7_HOLD` asks held; past that a bundle is dropped, as a node with
  depleted storage does. An ask is at most 4 MiB, however many pieces
  it arrives in.

## Configure

| env | default | what |
|---|---|---|
| `BP7_LISTEN` | `0.0.0.0:4556` | the UDP convergence-layer address, `ip:port` (IANA's 4556) |
| `BP7_NODE_EID` | `dtn://hum.local/inference` | this endpoint; a bundle for any other is not ours |
| `BP7_LABEL` | `main` | the bee's label; give each forager on one humd its own |
| `BP7_MODEL` | none | the model when an ask names none; with none, any nurse with room grows it |
| `BP7_ROUTE` | the address the bundle came from | where every answer goes: the router's UDPCL address, `host:port`, resolved once at start |
| `BP7_ALLOW` | anyone | the IPs a bundle is taken from, comma-separated (RFC 7122 §5) |
| `BP7_BLOOMS` | `4` | blooms grown at once |
| `BP7_MTU` | `1232` | the largest datagram an answer goes in; 1232 fits any IPv6 path |
| `BP7_SPOOL` | `$XDG_STATE_HOME/hum/hives/bp7/spool` | where asks are held |
| `BP7_HOLD` | `10000` | asks held at once |
| `BP7_SWEEP_MS` | `60000` | how often what has expired is forgotten |

Set any of them in the environment of `hum hive bp7 install`, which
hands them to the forager's service. Behind a router, set `BP7_ROUTE`: a
router sends from a port of its own choosing, not the one it listens on,
so the address a bundle came from is rarely where an answer can go.
Without a router, a node that sends from the socket it listens on gets
its answer there.

## Run

```bash
hum hive bp7 install                               # build it and keep it running; it enters its own bee
BP7_ROUTE=router.local:4556 hum hive bp7 install   # behind a router
```

The forager is a forager bee of the hive `bp7`; its hive must be
installed on the humd it lands on. For the ensemble to grow its asks on
another humd, that humd must be a peer, its nurse's hive installed here,
and this humd must trust remote nurses (`humd.trustRemoteNurses` in
`hum.json`, or `HUM_TRUST_REMOTE_NURSES=1`).

## Test

```bash
cd hives/bp7 && cargo test
```

The tests are the forager as a DTN node meets it, against a real humd:
`tests/dtn.rs` covers duplicates, bursts, expiry, fragments both ways,
corruption, status reports, loops, routing, restarts, an ask that names
no model, and the `cleanup` of each try's sid; `tests/outlive.rs`
covers humd going away; `tests/ensemble.rs` has an ask from space grown by
a nurse on another humd, over TCP; `tests/grower_gone.rs` is ignored
(below). `tests/common` holds a minimal DTN node, which is also the
shortest example of speaking to the forager.

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
- **No BPSec.** Anyone who can reach the UDP port can prompt this humd,
  and their blooms reach the tools attendants serve it; keep it behind a
  router, and set `BP7_ALLOW`. The `bp7` crate signs and verifies
  BIB-HMAC-SHA2 (RFC 9172, 9173) behind its `bpsec` feature, which this
  hive does not turn on.
- **No custody, no status reports.** The forager is a leaf: it sends no
  reception or delivery reports, even when an ask asks for them. The
  router around it does custody.
- **UDP only.** TCPCL and LTP are the router's to speak.
- **One answer per ask.** DTN answers are messages, not streams; the
  bloom's text unfurls are gathered into one answer.
- **humd's memory grows a little with every ask.** Every ask grows on a
  sid of its own, and after its `cleanup` humd keeps the sid's wane
  until it restarts and its sigil until the forager's connection closes.

## See also

- [RFC 9171](https://datatracker.ietf.org/doc/rfc9171/): Bundle Protocol v7
- [RFC 7122](https://datatracker.ietf.org/doc/rfc7122/): datagram convergence layers
- [RFC 9172](https://datatracker.ietf.org/doc/rfc9172/): BPSec
- [bp7-rs](https://github.com/dtn7/bp7-rs): the Rust crate this bee uses
- [NASA ION](https://sourceforge.net/projects/ion-dtn/) and [HDTN](https://github.com/nasa/HDTN): DTN routers
- [THRUM.md](../../THRUM.md): the protocol on humd's side

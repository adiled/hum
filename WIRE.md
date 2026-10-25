---
title: "thrum wire spec"
description: "the language-neutral protocol bees speak to humd. Implementable in any language with an NDJSON parser and a Unix socket library."
---

# thrum wire spec

> _the language-neutral protocol bees speak to humd. Implementable in any language with an NDJSON parser and a Unix socket library._

This is the **canonical wire spec**. hum speaks it through one client,
`hum-thrum`, in Rust: a Rust bee holds it in-process, and a bee in TS,
Python, Go or C reaches it through `hum bee link` with the client
codegen emits for its language (`thrum-clients/`). A language hum does
not generate a client for can speak [the link](#the-link) or the wire
itself. If a client disagrees with this document, the document wins.

## Transport

- **Framing**: NDJSON — one UTF-8-encoded JSON object per line,
  separated by `\n` (`0x0A`). No CRLF, no trailing whitespace,
  no leading BOM. Lines that don't parse as JSON are dropped silently
  by humd; clients should do the same.
- **Socket**: Unix stream socket at the first of
  - `$HUM_THRUM_SOCK` (explicit override), then `$HUM_SOCKET`
  - the `socket` named in `$XDG_STATE_HOME/hum/runtime.json`
  - `$XDG_STATE_HOME/hum/thrum.sock`
- **Encoding**: UTF-8. Keys are case-sensitive, camelCase by
  convention (the wire uses `protoVersion`, `sentAt`, etc.). The
  `chi` discriminator and all chi values are **kebab-case**
  (`"tendril-reach"`, `"drone-retrofit"`).
- **Backpressure**: humd reads at line rate; if a writer can't keep up,
  the receiver drops oldest unsent frames. Clients SHOULD assume
  at-most-once delivery of any single tone.
- **Authentication**: nothing on the socket itself. Filesystem
  permissions on the socket file are the trust boundary. The ensemble
  layer adds Ed25519 handshakes for inter-humd traffic — that lives on
  a different wire and is out of scope here.

## Frame structure (envelope)

Every tone is a JSON object with these top-level fields:

| key | type | required | meaning |
|---|---|---|---|
| `chi` | string | **yes** | Tone discriminator. Must be one of the kebab-case values in the chi registry below. |
| `rid` | string | **yes** | Request id. Echoed in correlated responses (e.g. `chi:"echo"`). Format-agnostic; hum's client mints a HumId (52 Crockford base32 characters) when a bee sends none. **Not a dedup key** — see [Identity](#identity). |
| `sid` | string | situational | The string a [sigil](#sigilsid) is drawn from: blooms whose tones carry it share that sigil. Required for `prompt`, `unfurl`, `wilt`, `tendril-reach`, and the like. The forager picks it. |
| `from` | string | situational | Sender identity. `HumdId` hex when crossing humds, bee name when on a local socket. |
| `to` | string | situational | Destination identity. `HumdId` hex for ensemble-routed tones; absent for local-only. |
| `sigil` | string | optional | 12-char content hash, see [Helpers](#helpers). Stable across reconnects for the same sid. |
| `wane` | integer | optional | Lamport clock per sigil — see [WaneTracker](#wanetracker). |
| `sentAt` | integer | optional | Wall-clock ms at send time. UTC. |
| `dusk` | integer | optional | Absolute ms expiry. humd drops a tone arriving with `now > dusk`, and counts it — see [Identity](#identity). |
| `mid` | string | optional | Originator-assigned message id. Presence is an at-most-once claim: a receiver that has already dispatched this `mid` drops the repeat. Distinct from `rid` — see [Identity](#identity). |
| `ext` | object | optional | Per-bee extension bag. Key it by your bee name; ignore other keys. |

Beyond the envelope, **each chi defines its own body fields**. A
`chi:"prompt"` tone carries `text`/`content`/`modelId`/`cwd`/etc.
Unknown body fields MUST be preserved on forwarding paths (gRPC
bridge, gossip relay) so future chi extensions are backward-compatible.

A key is **declared** when it is an envelope key or a field of its chi's
body struct in `thrum-core/src/views.rs`; those structs are the spec
(`thrum_core::spec`). Anything else is undeclared: no bee should send it
and no humd may rely on it. A bee's private data goes under `ext`. humd
checks every tone it hears and sends. In the field it logs each
undeclared key once (`thrum.tone.undeclared`); in the simulator the test
fails. A new key on the wire starts as a field in `views.rs`.

## Handshake

The first tone a nestler sends after `connect()` MUST be `chi:"hello"`.
**This tone is the registration**: the humd that receives it now knows
your bee exists, what chi values you speak, and can route tones
to you, provided the hive your hello names is installed on that humd
(`hum hive <ref> install`) and the hello is signed by a key that hive
entered (`hum bee enter --hive <hive> --role <role>`). A humd understands
a bee only through its hive, and knows it only by its key. The ensemble layer (peer humds, gossip) and the on-chain layer
(`HumdRegistry`) are additive opt-ins on top.

The handshake doesn't distinguish where the bytes came from. A hello
arriving on humd's local Unix socket and a hello bridged through the
ensemble from a peer humd hit the same code path. Local-dev runs,
local-prod-with-systemd setups, and distributed mesh deployments all
exercise the same `chi:"hello"` shape — see
[hives/foragers.md](../hives/) for the three deployment
paradigms (local-dev / local-prod / distributed) that share this
single protocol.

```json
{
  "chi": "hello",
  "rid": "hello-1",
  "hive": "<the hive you belong to>",
  "role": "nurse",
  "hid": "bee_<sha256 of your ed25519 pubkey>",
  "pubkey": "<hex ed25519 public key>",
  "signedAt": 1700000000000,
  "signature": "<hex ed25519 over hum-bee-socket-v1:<hid>:<signedAt>>",
  "version": "<your bee's semver>",
  "protoVersion": "<THRUM_VERSION you target>",
  "propensity": {
    "statefulness": "stateless | convention-stateful | stateful | transport-only",
    "richness":     "rich | medium | lean | opaque",
    "wire":         "<freeform tag, e.g. 'openai/chat-completions' or 'x402/tendril'>"
  },
  "chis": ["hello", "prompt", "unfurl", "wilt"],
  "source": "https://github.com/...optional pointer to your repo..."
}
```

- `hive`, `role`, `protoVersion`, `hid`, `pubkey`, `signedAt` and
  `signature` are **required**. The signature proves the hello comes from
  the entered key the `hid` names (see [hives/README.md](hives/README.md)).
- `role` is the one job the bee does, set when its hive entered it:
  `forager`, `nurse`, `builder` or `attendant` ([Roles](#roles)).
- `builds: true` on a forager's hello says its blooms build where it is:
  a prompt of its with no `cwd` builds in the home directory of its humd's
  host ([Where a bloom builds](#where-a-bloom-builds)).
- `chis` (note: plural) is the list of chi values this bee speaks
  and expects to receive. **Distinct from `chi`** at the top level —
  `chi` is the discriminator (this tone IS a hello); `chis` is the
  vocabulary (these are the tones this bee knows). One word per
  concept; never reuse `chi` for a list.
- All other fields — optional but **strongly recommended**: humd uses
  them to build a `BeeManifest` and gossip it to the rest of the
  ensemble on the `hum/hives/announce` topic.
- humd replies with `chi:"breath"` — a snapshot of any state relevant
  to this nestler (today: `{}`; reserved for future state sync).
- A hello with **no** `protoVersion` is **refused**: the bee is not
  registered, not announced to the ensemble, and the connection is closed.
- A hello naming **no** hive, or a hive **not installed** on this humd,
  is refused the same way. So is one that is **unsigned**, whose
  signature fails or is stale, whose key was never entered under that
  hive or has exited, or whose `role` is not the one its key entered for.
- A `protoVersion` whose **major** differs from humd's is **refused** the
  same way.
- A `protoVersion` that differs only in minor or patch is admitted, with a
  `thrum.hello.proto-drift` warning.
- An unparseable `protoVersion` is refused.

The bumping rules humd applies:
  - **patch** — docstring tweaks, additive-optional fields
  - **minor** — new chi value, new required field with compat path
  - **major** — removed chi, renamed chi, semantics changed

## The nest model

humd hosts no cells. A **nest** is the cells a hive hosts (live LLM
subprocesses) and the limits it keeps them under, turned on with
hum-nest. The wire never sees a nest as a thing of its own; it sees the
chi traffic that flows through it, and the `headroom` a nurse claims on
its hello and on each `chi:"pulse"` about its cells (`freeSlots`,
`totalSlots`). humd sends a prompt to a nurse with room, local first,
then on a peer, and to a full one only when no other can serve.

## Roles

A bee does one job, its `role`, set when its hive entered it. Direction
is sacred: the role says what the bee sends and what it hears, for the
whole of its life.

| role | does | sends | receives |
|---|---|---|---|
| **forager** | opens blooms, bringing the outside world into the thrum | `chi:"prompt"`, `chi:"cancel"`, `chi:"cleanup"`, `chi:"curate"`, `chi:"scout-result"`, `chi:"tendril-result"` for the tools it put on its prompt, and `chi:"tendril-reach"` straight to an attendant, with no bloom between | `chi:"unfurl"`, `chi:"wilt"`, `chi:"error"`, `chi:"pulse"`, `chi:"scout"`, `chi:"tendril-info"`, and `chi:"tendril-reach"` for its own tools |
| **nurse** | grows blooms in its cells; all inference is nursing | `chi:"unfurl"`, `chi:"wilt"`, `chi:"error"`, `chi:"pulse"`, `chi:"tendril-reach"`, `chi:"tendril-info"`, `chi:"scout"` | `chi:"prompt"`, `chi:"cancel"`, `chi:"cleanup"`, `chi:"curate"`, `chi:"tendril-result"`, `chi:"scout-result"` |
| **builder** | changes the world: runs what a bloom composes on its host, through [`bash`](#bash) | `chi:"tendril-result"` | `chi:"tendril-reach"` for `bash`, `chi:"cleanup"` |
| **attendant** | serves the fixed, named tools in its hello's `tools`, and nothing else | `chi:"tendril-result"`, or `chi:"error"` under the reach's `callId` to refuse it | `chi:"tendril-reach"` for its tools |

Every bee sends `chi:"hello"` first and hears `chi:"breath"` accept it.
It is the same bee throughout: a **nestler** before its hello is
accepted, a **nestled** after, and the role doesn't flip when the state
changes. **Nobody is two roles on the same connection.** A hive that
wants a bee for each job enters one for each.

The wire is **opaque to the cell's implementation.** A cell might
be a local subprocess, a Rust struct that wraps an
HTTP client to OpenAI's API, a deterministic mock for sim tests —
the wire sees the same petals unfurl back either way. The kind of
cell (`Nurse` impl, in Rust terms) is the hive's concern,
exposed to the wire only via `hive: "<kind>"` on hello.

What the wire *does* see, at the humd level:

- Each humd advertises in its `PeerCapabilities.nests` (gossiped via
  the ensemble) which **kinds** of hive its nest is configured to
  host.
- Each `chi:"prompt"` carries a `modelId`: the forager says which
  model it wants. humd's routing picks an appropriate nurse
  (a local one over thrum, or routes to a peer humd whose advertised
  hives can serve it).

That's the whole "compute discovery" mechanism. Nothing on a bee's
hello claims a nest: the kind is `hive: "<name>"`, and its `role`.
**The advertise lives on the humd, not invented per forager.**

If you want to add a new kind of compute to the mesh, you ship a
`Nurse` impl (any process speaking thrum), point it at a humd, and
the mesh discovers via the existing capability gossip; peers route
prompts your way. The wire stays exactly the same.

## Chi registry (THRUM_VERSION 0.8.0)

This section is the wire contract. `thrum-core/src/chi.rs` mirrors it as
the Rust enum, and `codegen` emits the TypeScript, Python, Go and C
mirrors from it — so the five copies cannot drift.

`body fields` below are the chi-specific fields only. `sid` is not
among them — it is an [envelope](#envelope) field, and a tone carries it
alongside any of these.

Each row carries two prose columns: `meaning` is for a human reading
this spec, `description` is the canonical one-liner the generated
clients carry as their doc comment. They are the same contract stated
at two lengths — `description` is the one the clients quote, so a chi
with no description here has none in the clients either.

### Nestler → humd

| chi | body fields | meaning | description |
|---|---|---|
| `hello` | `hive`, `role`, `protoVersion`, `hid`, `pubkey`, `signedAt`, `signature`, optional `builds`/`version`/`models`/`tools`/`propensity`/`chis`/`bind`/`headroom`/`source` | first frame after connect: announce self, signed | announce self: hive, role, signed identity |
| `prompt` | `sid`, `text`, `content` (petals), optional `modelId`/`cwd` (where it builds, `hum://`)/`systemPrompt`/`tools`/`allowedTools`/`resume`, and how the bloom grows: `temperature`/`prune`/`season`/`frost`/`seed`/`trellis`/`tendrils`/`sepal` | open a bloom | open a bloom: its petals, and how the cell should grow it |
| `cancel` | `sid` | stop the open bloom of this sigil | stop the open bloom |
| `cleanup` | `sid` | drop humd's state for this sigil | drop humd's state for a sigil |
| `curate` | `sid` | manual compaction request | manual compaction request |
| `scout-result` | `sid`, `callId`, `output` | a scout came back | a scout came back |
| `tendril-result` | `callId`, `result`/`output`, `isError`, `title`, `metadata` | what a tendril grasped | what a tendril grasped |
| `petal-cell` | `sid`, `cell` | OC message-graph update (graft hint) | OC message-graph update (graft hint) |

### humd → nestler

| chi | body fields | meaning | description |
|---|---|---|
| `breath` | `sigils` (state per sigil, usually `{}`), optional `protoVersion` | reply to hello: accepts it, and syncs state on connect | accept a hello, sync state on connect |
| `unfurl` | `petal` (its index in the bloom), `kind`, and what grew: `text`, `sap`, `callId`/`name`, `grain`, `propolis`, `source`, `brix`; `closed` once the petal is whole | one petal of a bloom growing | a petal growing |
| `wilt` | `reason`, `nectar`, optional `exitCode`, `subtype` | the bloom closes | close the bloom, with its nectar |
| `error` | `message`, `code`, optional `subtype`, `nectar` | the bloom broke off | the bloom broke off |
| `pulse` | `kind` (cell-spawned/cell-ready/cell-idle/cell-died/cell-evicted), optional `pid`/`headroom`, and on `cell-ready` the `cell`'s id, its `model` and `tools` | a cell's life, with the nurse's headroom after it | a cell's life, and headroom |
| `scout` | `task`, `tools` | send a scout out | send a scout out |
| `tendril-reach` | `sid`, `callId`, `name`, `args`, and on a `bash` reach to a builder the `cwd` its shell starts in, its host's home directory when absent | a brokered tool call reaching | a tendril reaching |
| `tendril-meta` | `callId`, `name`, `metadata` | what a tendril carries besides its result | what a tendril carries besides its result |
| `tendril-info` | `callId`, `name`, `args`, `result`, `isError` | a tendril that already reached and came back, told whole. Unlike `tendril-reach` and `tendril-result`, a two-phase handoff the receiver acts on, `tendril-info` says "this ran upstream, here is all of it", and a receiver only shows it. | a tendril that already came back, told whole, for showing only |

### Either direction

| chi | body fields | meaning | description |
|---|---|---|
| `echo` | `ok`, optional `error` | delivery ack for `rid` | delivery ack for a rid |
| `perf-mark` | `label`, `phase`, `t` | a timing measurement, measured both ways | a timing measurement — measured both ways |
| `log` | `level`, `message`, optional fields | structured log forwarding | structured log forwarding |
| `drone` | (drone-shaped payload) | drone heartbeat | drone heartbeat |
| `drone-retrofit` | (retrofit instruction) | drone swallow + retry signal | drone swallow + retry signal |

### Ensemble / inter-humd plumbing

Only emitted across the ensemble layer (not by local nestlers).
See [`ensemble/README.md`](../ensemble/README.md).

| chi | body fields | meaning | description |
|---|---|---|
| `peer-add` | `humd_id`, `hints` | register a peer humd | register a peer humd — `{ humd_id: hex, hints: [..] }` |
| `peer-remove` | `humd_id` | drop a peer humd | drop a peer humd — `{ humd_id: hex }` |
| `attach` | `sid`, `to`, `from`, `hearOnly` | peer humd asks to observe a sid hosted here. The host records `from` as an observer of `sid`; reply tones for that sid fan out to every observer in addition to the prompt origin. | peer humd asks to observe a sid hosted here — `{ sid, to, from, hearOnly }`. The host records `from` as an observer of `sid`; reply tones for that sid fan out to every observer in addition to the prompt origin. |
| `detach` | `sid`, `to`, `from` | peer humd stops observing. The host drops `from` from the observer roster for `sid`. | peer humd stops observing — `{ sid, to, from }`. The host drops `from` from the observer roster for `sid`. |
| `wane-sync` | `from`, `snapshot` | reconcile WaneTracker after a partition heal, where `snapshot` maps sigil to wane value. The receiver merges each entry by taking the max of local and remote (wane is a Lamport clock; max is convergent). No reply is required — both sides emit their snapshot on heal, so each is informed exactly once. | reconcile WaneTracker after a partition heal — `{ from, snapshot }` where `snapshot` is a map of sigil → wane value. The receiver merges each entry by taking the max of local and remote (wane is a Lamport clock; max is convergent). No reply is required — both sides emit their snapshot on heal, so each is informed exactly once. |
| `gossip-publish` | `topic`, `payload`, `from`, `msg_id` | ensemble-wide gossip pub-sub message. Fan-out broadcast above the Transport layer: every PeerConnection is a gossip neighbor. The receiver dedups on `msg_id` using a bounded LRU, dispatches to per-topic subscribers, and re-fans the tone to every OTHER installed peer so the message percolates across the mesh. Distinct from unicast `route`, which targets ONE humd — gossip is mesh-wide announcements: hum relocation, humd overload, drone alerts. `msg_id` is assigned by the ORIGINATOR, once per publish, and is not derived from `rid` or the payload. A repeat of the same content is a new message; only a byte-identical retransmission is a duplicate, and only the sender can tell those apart. | ensemble-wide gossip pub-sub message — `{ topic, payload, from, msg_id }`. Fan-out broadcast above the Transport layer: every PeerConnection is a gossip neighbor. The receiver dedups on `msg_id` using a bounded LRU, dispatches to per-topic subscribers, and re-fans the tone to every OTHER installed peer so the message percolates across the mesh. Distinct from unicast `route` (which targets ONE humd) — gossip is mesh-wide announcements: hum relocation, humd overload, drone alerts.  `msg_id` is assigned by the ORIGINATOR, once per publish, and is not derived from `rid` or the payload. A repeat of the same content is a new message; only a byte-identical retransmission is a duplicate, and only the sender can tell those apart. |
| `kad-find-node` | `query_id`, `target`, `from` | Kademlia DHT FIND_NODE query. The receiver answers with `kad-find-node-resp` carrying up to K HumdAddrs from its routing table closest in XOR distance to `target`. Sent during `Ensemble::kad_find` iterative lookups when a peer's HumdAddr isn't already known locally. | Kademlia DHT FIND_NODE query — `{ query_id, target: <Hid hex>, from: <Hid hex> }`. The receiver answers with `kad-find-node-resp` carrying up to K HumdAddrs from its routing table closest in XOR distance to `target`. Sent during `Ensemble::kad_find` iterative lookups when a peer's HumdAddr isn't already known locally. |
| `kad-find-node-resp` | `query_id`, `from`, `closest` | Kademlia DHT FIND_NODE response, matched to the originating `kad-find-node` by `query_id`. The lookup driver inserts every advertised HumdAddr into its routing table and re-queries the α closest unqueried peers until no closer node is returned. | Kademlia DHT FIND_NODE response — `{ query_id, from: <Hid hex>, closest: [<HumdAddr JSON>, ...] }`. Matched to the originating `kad-find-node` by `query_id`. The lookup driver inserts every advertised HumdAddr into its routing table and re-queries the α closest unqueried peers until no closer node is returned. |
| `backfill` | `author`, `from` | thehum chi-log replay request. Host humd answers with one `chi:"backfill-event"` tone per event in `[from, ..)` for the named author. | thehum chi-log replay request — `{ author: <hid>, from: <seq> }`. Host humd answers with one `chi:"backfill-event"` tone per event in `[from, ..)` for the named author. |

### `pulse.kind`

`pulse` tones carry a `kind` from its own enum.

| kind | description |
|---|---|
| `cell-spawned` | process created |
| `cell-ready` | brooded: accepting prompts, its id, model and tools known |
| `cell-idle` | the bloom wilted, no listeners |
| `cell-died` | process exited |
| `cell-evicted` | killed to make room |

### `petal.kind`

A petal is one unit of content: in a prompt's `content`, and as it grows
in an `unfurl`.

| kind | description |
|---|---|
| `text` | words, in `text` |
| `tendril` | a brokered tool call: `callId`, `name`, `args`; its input grows as `sap` |
| `grasp` | what a tendril came back with: `callId`, `result`, `isError` |
| `pollen` | a grain of media: `grain` with its `kind` (image, audio, file), `mediaType`, and `data` or `url` |
| `sepal` | the cell's reasoning, in `text`, sealed by `propolis` so it can carry into the next bloom |
| `waggle` | where a petal's words came from: `source` |
| `thorn` | the cell declining, or a filter acting: `text` says why |
| `brix` | how likely each token was: `brix` |
| `bouquet` | several candidate petals for one prompt: `bouquet` |

### How a bloom grows

A prompt says how the cell should grow its bloom.

| field | meaning |
|---|---|
| `temperature` | how warm the sampling is |
| `prune` | `p` and `k`: cut the unlikely tail |
| `season` | how many tokens the bloom may grow |
| `frost` | where the bloom stops |
| `seed` | a seed, for a bloom grown again the same |
| `trellis` | the shape the answer grows along: a schema, or `"json"` |
| `tendrils` | `reach` (`auto`, `none`, `required`, or a tendril's name) and `parallel` |
| `sepal` | how much the cell reasons first: `effort`, or a `budget` |

### Advisories

humd refuses what it must (an unsigned hello, a bee its hive did not
enter, a revoked key, an incompatible protocol) and advises on the rest.
An advisory reaches the bee as a `log` tone whose `fields.advisory` is
one of these codes, and hum's client raises some itself before a tone
leaves the bee. humd keeps them per bee in `advisories.json` in the state
root, and `hum doctor` lists them.

| code | raised by | meaning |
|---|---|---|
| `undeclared-key` | client, humd | a tone carries keys the thrum does not declare for its chi |
| `unknown-chi` | client | a tone's chi is not in the thrum |
| `not-a-tone` | client | a line from the bee is not a JSON object with a chi, and was dropped |
| `client-owned` | client | the bee set a hello field the client owns, and it was ignored |
| `tone-refused` | client | humd refused a tone under a rid the client gave it |
| `dropped-while-away` | client | tones sent while humd was away were dropped |
| `proto-drift` | humd | the bee speaks another minor version of the thrum |
| `reconnect-storm` | humd | the bee said hello 10 times within 10 minutes |
| `did-not-return` | humd | the bee was nestled when humd stopped and had not come back 2 minutes after humd started |
| `cwd-not-hum` | humd | a prompt's `cwd` is not a `hum://` address, and was dropped |

**Send deadline.** Every inter-humd send is bounded at 5s
(`ensemble::SEND_TIMEOUT`). A peer that stops reading fills its socket
buffer rather than closing, so the write stalls indefinitely while the
connection still looks healthy to a liveness lease. On timeout humd
closes that connection, counts the event in `send_timeouts`, and lets
the lease mark the peer dead — a bounded send is what keeps one stalled
peer from wedging a fan-out to every other peer. Implementations that
speak this wire should impose an equivalent deadline; a peer that wants
larger tones or a slower link should say so rather than write without
one.

## Helpers

Three deterministic helpers are pinned by the protocol. `thrum-core`
holds them, and anything else that computes them MUST match
byte-for-byte.

### `sigil(sid)`

```
sigil = lowercase_hex(sha256("sigil:" + sid)[..6])      // 12 chars
```

What ties a run of blooms together, the same on every humd. humd routes a
bloom's tones by it and keys its wane by it; `sid` is the string the
forager chose to draw it from.

### `rid()`

```
rid = crockford_base32(now_ms[48 bits] ++ random[208 bits])   // 52 chars, a HumId
```

Correlation id, ordered by when it was minted. Format-agnostic on
receive: only the originator's correlation logic cares about the exact
format.

### Identity

`rid` is correlation, not identity. Three things follow, and all three
are load-bearing:

- A request and its correlated response share one `rid`. `chi:"echo"`
  is the ack *for* an `rid`, so a receiver that deduped on `rid` would
  drop every response as a duplicate of its request.
- `rid` is explicitly format-agnostic and per-originator, so two humds
  can mint the same value. Some bees mint per-session values
  (`p-<sid>`, `prompt-<sid>`) that recur across restarts.
- A sender that means to publish the same content twice is not
  transmitting a duplicate. Repeats are ordinary for the tones gossip
  carries — a heartbeat, a standing overload alert, a retry.

So a message that needs at-most-once delivery carries its own
originator-assigned id, minted once per publish and never reused:

| chi | id field | who mints it |
|---|---|---|
| `gossip-publish` | `msg_id` | originator, once per publish |
| any, when at-most-once is wanted | `mid` | originator, once per logical message |

Receivers dedup on that field and never mint one themselves. The
reference `Ensemble::publish` mints `"{origin6}-{ms:x}-{seq:x}"`: a
per-process counter makes each publish distinct regardless of clock or
content, and the origin prefix keeps two humds from colliding.

`mid` extends the same rule to unicast, and the distinction is the
whole design. `mid` answers *which message is this*; `rid` answers
*which conversation is it in*. A response therefore carries the
request's `rid` and its own `mid` — same rid, different mid, both
delivered. That is what a retry looks like too: same rid, same mid,
delivered once.

`mid` is optional, and its absence is meaningful rather than a
shortcoming. A tone with no `mid` makes no at-most-once claim and is
delivered every time it is sent, so a sender may repeat a body as often
as it likes. Suppressing those would be the original mistake in another
guise — dropping a message the sender meant to send. A receiver must
not substitute `rid` for a missing `mid`.

Each receiver enforces `mid` and `dusk` at its own edge, once per hop,
and keeps the two independent:

- `dusk` is checked first, so a dead tone never occupies seen-set
  capacity and cannot displace a live id.
- The `mid` seen-set is separate from the gossip `msg_id` seen-set. A
  `mid` records that this node already *delivered* a tone; a `msg_id`
  also governs whether it is *re-fanned*. Sharing one set would couple
  two unrelated decisions and let one evict the other's entries early.

Content-addressing an id (`sha256(topic:rid:from:payload)`) cannot
work here. It conflates "delivered twice" with "sent twice", and since
a sender typically marks its own publish seen before sending, a repeat
that hashed alike was dropped before it ever left the origin.

`dusk` bounds the window a duplicate can be recognised in at all: a
tone past its `dusk` is dropped on arrival rather than delivered, so
the seen-set only has to remember ids for as long as they can still be
legitimately in flight. A tone with no `dusk` can be re-fanned
arbitrarily late, so such ids are bounded by the seen-set cap instead
of by any deadline.

### `WaneTracker`

```
wane[sigil] = u64 counter starting at 0
tick(sigil): wane[sigil] += 1; return wane[sigil]
behind(sigil, remote): remote > wane[sigil]
```

One Lamport clock per sigil. Local mutations call `tick`; the new
value rides on the next outgoing tone in `wane`. On receive:

```
if remote_wane > local_wane[sigil]:
    request resync (wane-sync)
    local_wane[sigil] = max(local_wane[sigil], remote_wane)
```

Both ends converge in O(1) round-trips after the link heals.

## Tendrils

A tendril is a tool call a cell makes, brokered across the wire. A nurse
reaches (`chi:"tendril-reach"`), and the bee that serves the tool answers
with `chi:"tendril-result"` carrying the same `callId`. Three kinds of
bee serve one:

- a **builder** serves [`bash`](#bash), the thrum's own tendril;
- an **attendant** serves the tools it lists in its hello's `tools`;
- a **forager** serves the tools it put on its prompt's `tools`.

When humd hands a nurse a prompt, the prompt's `tools` are everything the
bloom may reach, each name once and the first to claim it keeping it:
`bash` when the bloom builds somewhere, every attendant's here, the
forager's own, and every attendant's on a live peer, when this humd
trusts remote attendants and knows the peer bee's hive. A nurse hands
them to its cell as they are. humd routes each reach by its name: `bash`
to the builder where the bloom builds, any other name to an attendant
serving it here, then back to the forager when it offered the name, then
to the peer whose attendant serves it.

A peer's attendant sees the input of every reach sent to it, and its
tools' descriptions reach every model here, so trusting remote
attendants is a choice, off by default: `humd.trustRemoteAttendants` in
hum.json, or `HUM_TRUST_REMOTE_ATTENDANTS`. A reach that came from a peer
arrives at the attendant with that peer's `from`, so an attendant can
answer its own humd's blooms and not a peer's.

```
forager → humd:   chi:"prompt" { tools: [{ name, description, inputSchema }, ...] }
humd → nurse:     chi:"prompt" { tools: [...the forager's, the attendants', bash] }
nurse → humd:     chi:"tendril-reach" { sid, callId, name, args }
humd → server:    chi:"tendril-reach" { sid, callId, name, args, cwd? }
server → humd:    chi:"tendril-result" { sid, callId, output, isError }
humd → nurse:     chi:"tendril-result" { sid, callId, output, isError }
```

A reach no bee can serve is answered by humd itself, with a
`tendril-result` whose `isError` is true and whose `output` says why, so
the cell does not wait for an answer that cannot come.

### `bash`

The one tendril the thrum declares (`thrum_core::bash_tool`), so every
nurse hands its cell the same tool. A reach carries `command` and an
optional `timeout` in ms (120000 by default, 600000 at most). The builder
runs it in the sigil's shell, which starts where the bloom builds and
keeps its directory and environment from one reach to the next, until
the sigil's `cleanup`. The result's `output` is stdout and stderr
together, its last line naming the exit code when it is not 0;
`metadata.exitCode` is the exit code, and `isError` is true when it is
not 0.

### Where a bloom builds

A prompt's `cwd` says where its bloom builds: `hum://<humdid>/<path>`,
the builder on that humd, its shell starting in `<path>`. The host may be
a full hid, a short one, or an alias from the forager's humd's
`peers.json`; the forager's humd resolves it to a full hid before the
prompt goes anywhere.

- `hum://<humdid>` with no path starts in the home directory of that
  host.
- `hum:///<path>`, with no host, is the forager's own humd.
- No `cwd` builds in the home directory of the forager's own humd, but
  only when the forager's hello says `builds: true`. Otherwise the bloom
  builds nowhere, and its `bash` reaches are answered as unserved.
- A `cwd` that is not a `hum://` address is dropped, with the
  `cwd-not-hum` advisory, and the prompt is taken as having none.

## The client

A bee does not implement this wire; it uses hum's client. In Rust that
is `hum_thrum::Bee::link`. In TS, Python, Go and C it is the client in
`thrum-clients/<lang>`, generated from the Rust, which runs
`hum bee link --hive <hive> --as <label> --role <role>` and talks to it
over two pipes. Either way the same Rust decides everything below, so every
language behaves the same, and `cargo test -p hum --test conform` holds
each of them to it against a real humd.

### The link

One JSON object per line, each way. A line with a `chi` is a tone and
passes through unchanged; a line without one is the link's own.

| direction | line | meaning |
|---|---|---|
| link to bee | `{"link":"ready","linkVersion":2,"thrum":..,"bee":..,"hive":..,"as":..,"role":..}` | first: the bee is entered; a client of another `linkVersion` stops here |
| bee to link | `{"hello":{..}}` | first: the bee's own hello fields; the link connects once it has them |
| link to bee | `{"link":"nestled"}` | humd admitted the bee, at start and after every reconnect |
| link to bee | `{"link":"astray"}` | humd went away; the link is dialing again |
| link to bee | `{"link":"advisory","code":..,"message":..,"from":..,"fields":{..}}` | something to fix ([Advisories](#advisories)) |
| bee to link | `{"hello":{..}}` | later: a change to the hello; a null drops a field |
| link to bee | `{"link":"refused","error":..}` | last: humd refused the bee, or its hive is not installed |

The link owns the hello's `chi`, `rid`, `hive`, `role`, `protoVersion`,
`hid`, `from`, `pubkey`, `signedAt` and `signature`, gives a tone without a
`rid` one, and swallows humd's acknowledgements of the rids it minted.
It exits 0 when the bee closes its stdin, 3 when the bee is refused, 2
on a first line that is not a hello, and 1 otherwise. It ignores
interrupts and terminations: its life is the pipe's.

### Speaking the wire yourself

A bee in a language hum does not generate a client for can speak the
link, or the wire itself. The wire, as hum's client speaks it:

1. Resolve the socket as [Transport](#transport) says, afresh on every
   dial.
2. Connect. humd breathes once (`sigils: []`) before it has heard
   anything; that breath admits nothing.
3. Send the hello, signed as a key the bee's hive entered, under a fresh
   `rid`. humd acknowledges it (`echo`, `ok: true`), then gives its
   verdict: a breath (`sigils: {}`) admits the bee, an `echo` with
   `ok: false` under the hello's `rid` refuses it. A refusal is final,
   even one that comes after admission. A change to the hello is a
   fresh signed hello on the same connection: at once while nestled,
   and the moment the bee is admitted when the change came while the
   verdict was pending.
4. Split incoming bytes on `\n`, parse each line, drop what does not
   parse. Dispatch by `sid`, then by `chi`.
5. Send tones one per line. Give every tone a `rid`. Check it against
   the views first: humd advises on what it does not declare.
6. Outlive humd. When the socket closes, reconnect with backoff (250 ms,
   doubling to 30 s, with jitter) and send a freshly signed hello,
   carrying the latest `headroom`, including any the bee claimed while
   humd was away. Drop the rest of what the bee sends while humd is
   away. A bee that will not reconnect must exit non-zero, so its
   supervisor starts it again; humd advises `did-not-return` on a bee
   that does neither.

## Version history

| THRUM_VERSION | what changed |
|---|---|
| 0.8.0 | hello names its `hive` and its one `role` (`forager`, `nurse`, `builder`, `attendant`; was `roles`, and before that `bee`), both required, and is signed by a key its hive entered for that role; bee hids are `bee_`; `sigil(sid)` drops its nest; a nurse claims `headroom` on hello and `pulse`; humd hosts no cells; `permission-ask` and `release-permit` are gone, and the drone `load`'s `pending_permissions` with them. The thrum declares `bash`, which a builder serves; a prompt's `cwd` is where its bloom builds, `hum://`, and a forager's hello may say `builds`; humd hands a nurse the bloom's tools in the prompt's `tools`, so `provides`, `foragerTools`, `provided` and `disallowedTools` are gone. The wire speaks hum's building blocks: `chunk` is `unfurl` (a petal growing, by kind), `finish` is `wilt`, `usage` is `nectar`, `tool-*` are `tendril-*`, subagents are `scout`s, `session-ready` folds into the `cell-ready` pulse, breath carries `sigils`; prompt `content` is petals, and a prompt says how its bloom grows |
| 0.7.0 | ensemble plumbing chi (peer-add/remove, attach/detach, wane-sync, gossip-publish, kad-find-node, kad-find-node-resp) |
| 0.6.0 | drone, drone-retrofit, perf-mark first-classed |
| 0.5.0 | release-permit, permission-ask, tendril-reach, tendril-result |
| 0.4.0 | tool-call / tool-result / tool-meta first-classed |
| 0.3.0 | breath / pulse / session-ready handshake refinement |
| 0.2.0 | hello / prompt / chunk / finish baseline |

## See also

- [`thrum-core`](../thrum-core): the Rust source of truth (chi enum, views, helpers).
- [`hum-thrum`](../hum-thrum): the client, sans-IO at its core, and the link.
- [`thrum-clients`](../thrum-clients): the generated client for TS, Python, Go and C.
- [`ensemble/README.md`](../ensemble/README.md) — inter-humd routing.
- [`hives/foragers.md`](../hives/foragers.md) — typology + propensity axes.
- [adiled.github.io/hum](https://adiled.github.io/hum/) — docs site.

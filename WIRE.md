---
title: "thrum wire spec"
description: "the language-neutral protocol bees speak to humd. Implementable in any language with an NDJSON parser and a Unix socket library."
---

# thrum wire spec

> _the language-neutral protocol bees speak to humd. Implementable in any language with an NDJSON parser and a Unix socket library._

This is the **canonical wire spec**. Rust (`thrum-core`), TypeScript
(`thrum-clients/ts`), Python (`thrum-clients/python`), Go
(`thrum-clients/go`) and C (`thrum-clients/c`) are reference clients — they all conform to the
rules below. If a client disagrees with this document, the document
wins.

## Transport

- **Framing**: NDJSON — one UTF-8-encoded JSON object per line,
  separated by `\n` (`0x0A`). No CRLF, no trailing whitespace,
  no leading BOM. Lines that don't parse as JSON are dropped silently
  by humd; clients should do the same.
- **Socket**: Unix stream socket at one of (in priority order)
  - `$HUM_THRUM_SOCK` (explicit override)
  - `$XDG_STATE_HOME/hum/thrum.sock`
  - `/run/user/<uid>/hum/thrum.sock`
- **Encoding**: UTF-8. Keys are case-sensitive, camelCase by
  convention (the wire uses `protoVersion`, `sentAt`, etc.). The
  `chi` discriminator and all chi values are **kebab-case**
  (`"tool-call"`, `"release-permit"`).
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
| `rid` | string | **yes** | Request id. Echoed in correlated responses (e.g. `chi:"echo"`). Format-agnostic; reference clients use `"{base36-ms-timestamp}-{base36-counter}"`. **Not a dedup key** — see [Identity](#identity). |
| `sid` | string | situational | Session id. Required for `prompt`, `chunk`, `finish`, `tool-call`, etc. Picked by the originator. |
| `from` | string | situational | Sender identity. `HumdId` hex when crossing humds, bee name when on a local socket. |
| `to` | string | situational | Destination identity. `HumdId` hex for ensemble-routed tones; absent for local-only. |
| `sigil` | string | optional | 12-char content hash, see [Helpers](#helpers). Stable across reconnects for the same (nest, sid). |
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
entered (`hum bee enter --hive <hive>`). A humd understands a bee only
through its hive, and knows it only by its key. The ensemble layer (peer humds, gossip) and the on-chain layer
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
  "roles": ["worker"],
  "hid": "bee_<sha256 of your ed25519 pubkey>",
  "pubkey": "<hex ed25519 public key>",
  "signedAt": 1700000000000,
  "signature": "<hex ed25519 over hum-bee-socket-v1:<hid>:<signedAt>>",
  "version": "<your bee's semver>",
  "protoVersion": "<THRUM_VERSION you target>",
  "propensity": {
    "statefulness": "stateless | convention-stateful | stateful | transport-only",
    "richness":     "rich | medium | lean | opaque",
    "wire":         "<freeform tag, e.g. 'openai/chat-completions' or 'x402/tool-call'>"
  },
  "chis": ["hello", "prompt", "chunk", "finish"],
  "source": "https://github.com/...optional pointer to your repo..."
}
```

- `hive`, `protoVersion`, `hid`, `pubkey`, `signedAt` and `signature`
  are **required**. The signature proves the hello comes from the entered
  key the `hid` names (see [hives/README.md](hives/README.md)).
- `roles` is what the bee claims to do: `worker`, `forager`, both, or
  a role of its own. It is a claim, not identity. Older bees sent it
  as `bee`; humd still reads that.
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
  signature fails or is stale, or whose key was never entered under that
  hive or has exited.
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
chi traffic that flows through it, and the `headroom` a worker claims on
its hello and on each `chi:"pulse"` about its cells (`freeSlots`,
`totalSlots`). humd sends a prompt to a worker with room, local first,
then on a peer, and to a full one only when no other can serve.

Direction is sacred. Two ends of the same connection, two roles:

| who | sends | receives |
|---|---|---|
| **asker side** (a nestler when pre-handshake, a nestled after) | `chi:"hello"` (first ask), then `chi:"prompt"`, `chi:"cancel"`, `chi:"tool-result"`, `chi:"release-permit"`, `chi:"cleanup"`, `chi:"curate"` — keeps asking the whole lifetime | `chi:"breath"` (accepting the handshake), `chi:"chunk"`, `chi:"finish"`, `chi:"tool-call"`, `chi:"permission-ask"`, `chi:"session-ready"`, `chi:"pulse"` |
| **cell side** (compute) | `chi:"chunk"`, `chi:"finish"`, `chi:"tool-call"`, `chi:"permission-ask"` | `chi:"prompt"`, `chi:"tool-result"`, `chi:"release-permit"`, `chi:"cancel"` |

The asker is the same actor throughout — a **nestler** before its
hello is accepted, a **nestled** after. The role doesn't flip when
the state changes; "hello" is just the first of many asks the
connection will carry. Cancels, prompts, tool-results, cleanups all
flow from the nestled state, not just hello from the nestler.

A cell is always answerer. **Nobody is both on the same connection.**
A process that wanted to "also offer compute" would not do so by
inverting its nestler connection — it would do so by being a cell
inside *some humd's nest*, addressable via that humd's ensemble
advertise.

The wire is **opaque to the cell's implementation.** A cell might
be a local subprocess (`claude-cli`), a Rust struct that wraps an
HTTP client to OpenAI's API, a deterministic mock for sim tests —
the wire sees identical chunks coming back either way. The kind of
cell (`Worker` impl, in Rust terms) is the hive's concern,
exposed to the wire only via `hive: "<kind>"` on hello.

What the wire *does* see, at the humd level:

- Each humd advertises in its `PeerCapabilities.nests` (gossiped via
  the ensemble) which **kinds** of hive its nest is configured to
  host (e.g. `["claude-cli", "ollama-local"]`).
- Each `chi:"prompt"` carries a `modelId` — the asker says which
  model it wants. humd's routing picks an appropriate worker bee
  (a local one over thrum, or routes to a peer humd whose advertised
  hives can serve it).

That's the whole "compute discovery" mechanism. No `provides_nest`
field on a bee's hello — the kind is `hive: "<name>"` + bee role flag.
**The advertise lives on the humd, not invented per-asker.**

If you want to add a new kind of compute to the mesh, you ship a
`Worker` impl (any process speaking thrum), point it at a humd, and
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

### Nestler → daemon

| chi | body fields | meaning | description |
|---|---|---|
| `hello` | `hive`, `protoVersion`, `hid`, `pubkey`, `signedAt`, `signature`, optional `roles`/`version`/`models`/`tools`/`provides`/`propensity`/`chis`/`bind`/`headroom`/`source` | first frame after connect: announce self, signed | announce self: hive, roles, signed identity |
| `prompt` | `sid`, `text`/`content`, optional `modelId`/`cwd`/`systemPrompt`/`tools` | start a turn | start a turn — content, system, tools |
| `cancel` | `sid` | interrupt the current turn for this sid | interrupt mid-turn |
| `cleanup` | `sid` | drop daemon state for this session | session deleted, drop daemon state |
| `curate` | `sid` | manual compaction request | manual compaction request |
| `release-permit` | `callId`, `ok`, optional `error` | answer a `permission-ask` | resolve an earlier permission-ask |
| `tendril-result` | `sid`, `callId`, `result` | task subagent answered | task subagent answered |
| `tool-result` | `callId`, `result`/`output`, `isError`, `title`, `metadata` | nestler-declared tool answered | nestler-declared tool answered |
| `petal-cell` | `sid`, `cell` | OC message-graph update (graft hint) | OC message-graph update (graft hint) |

### Daemon → nestler

| chi | body fields | meaning | description |
|---|---|---|
| `breath` | (state snapshot, usually `{}`) | reply to hello — handshake, full state sync on connect | handshake — full state sync on connect |
| `chunk` | `chunkType`, `blockIdx`, `delta`, `partialJson` | streamed model output, partwise (text/reasoning/tool) | model output partwise (text/reasoning/tool) |
| `finish` | `finishReason`, `usage`, optional `exitCode`, `subtype` | turn complete | turn complete — finishReason + usage |
| `error` | `message`, `code`, optional `subtype`, `usage` | turn aborted / hard error | turn aborted |
| `session-ready` | `nestId`, `model`, `tools` | nest spawned, ready for prompts | nest spawned, claude session id known |
| `pulse` | `kind` (CellSpawned/CellReady/CellIdle/CellDied/CellEvicted), optional `pid`/`headroom` | cell lifecycle event, with the worker's headroom after it | cell lifecycle event and headroom |
| `permission-ask` | `callId`, `toolName`, `message`, `arg` | mid-stream permission needed | mid-stream permission needed |
| `tendril-reach` | `task`, `tools` | task subagent dispatch | task subagent dispatch |
| `tool-call` | `sid`, `callId`, `name`, `args` | nestler-declared tool dispatch | nestler-declared tool dispatch |
| `tool-meta` | `callId`, `toolName`, `metadata` | out-of-band metadata for a tool result | out-of-band metadata for a tool result |
| `tool-info` | `callId`, `name`, `args`, `result` | completed-in-server tool execution, purely informational. Carries the full pair as one tone. Unlike `tool-call`/`tool-result`, which form a two-phase handoff requiring the recipient to execute and reply, `tool-info` says "this tool ran upstream, here's everything." Consumers render it without taking action. | completed-in-server tool execution — purely informational. Carries the full {callId, name, args, result} pair as one tone. Unlike `ToolCall`/`ToolResult` (which form a two-phase handoff requiring the recipient to execute and reply), `ToolInfo` says "this tool ran upstream, here's everything." Consumers render it without taking action. Used by humd when its native MCP tools run in-process, or when a nestler-tool roundtrip completes successfully and observers want a clean audit event. |

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
| `cell-ready` | system init received, accepting input |
| `cell-idle` | turn complete, no listeners |
| `cell-died` | process exited |
| `cell-evicted` | killed to make room |

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

The reference clients ship three deterministic helpers. Algorithms
are pinned by the protocol — any client implementation MUST match
byte-for-byte.

### `sigil(sid)`

```
sigil = lowercase_hex(sha256("sigil:" + sid)[..6])      // 12 chars
```

Stable identifier for a session, the same on every humd. Used by humd's
WaneTracker keys and by drift detection; `sid` is the session id the
asker chose.

### `rid()`

```
rid = base36(now_ms) + "-" + base36(counter++)
```

Monotonic correlation id. Counter is per-process and starts at 0.
Format-agnostic on receive — only the originator's correlation logic
cares about the exact format.

### Identity

`rid` is correlation, not identity. Three things follow, and all three
are load-bearing:

- A request and its correlated response share one `rid`. `chi:"echo"`
  is the ack *for* an `rid`, so a receiver that deduped on `rid` would
  drop every response as a duplicate of its request.
- `rid` is explicitly format-agnostic and per-originator, so two humds
  can mint the same value. Reference clients mint per-session values
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

## Tool-call extension

A nestler-declared tool is an arbitrary callable the bee exposes
to the model via `chi:"prompt"` `tools` array. When the model picks
it, humd emits `chi:"tool-call"`; the nestler answers with
`chi:"tool-result"` carrying the same `callId`.

```
nestler → daemon: chi:"prompt" { tools: [{ name, description, input_schema }, ...] }
daemon → nestler: chi:"tool-call" { sid, callId, name, args }
nestler → daemon: chi:"tool-result" { sid, callId, result }
```

Errors set `result.error` and humd surfaces them as a model error
turn. Timeouts: if no `tool-result` arrives within the nestler-side
timeout, send `chi:"tool-result"` with `result.error = "timeout"` so
the model unblocks.

## Implementing a client — checklist

A clean port to a new language takes ~80 LoC. The minimum:

1. Resolve the socket path: `HUM_THRUM_SOCK`, then `HUM_SOCKET`, then the
   `socket` named in `$XDG_STATE_HOME/hum/runtime.json`, then
   `$XDG_STATE_HOME/hum/thrum.sock`.
2. `connect()` over a Unix stream socket. Buffer incoming bytes,
   split on `\n`, JSON-parse each line, drop on parse error.
3. Send `chi:"hello"` immediately after connect. Buffer subsequent
   writes until connect resolves.
4. Multiplex by `sid`: keep a `Map<sid, handler>` plus one catch-all
   handler for tones without a sid (breath, echo, pulse).
5. Serialize outbound tones with newline termination (`json.dumps(t) + "\n"`).
6. On close: drop any pending writes, surface the disconnect to handlers.
7. Outlive humd. humd restarts (an upgrade, a crash, a reboot), so when
   the socket closes, reconnect with backoff (250 ms, doubling to 30 s)
   and send a freshly signed hello. A refused hello is final. A bee that
   does not reconnect must exit non-zero, so whatever supervises it
   starts it again; exiting 0 reads as done.

Reference: [`thrum-core`](../thrum-core),
[`thrum-clients/ts`](../thrum-clients/ts),
[`thrum-clients/python`](../thrum-clients/python),
[`thrum-clients/go`](../thrum-clients/go),
[`thrum-clients/c`](../thrum-clients/c).

## Version history

| THRUM_VERSION | what changed |
|---|---|
| 0.8.0 | hello names its `hive` (required) and `roles` (was `bee`), and is signed by a key its hive entered; bee hids are `bee_`; `sigil(sid)` drops its nest; a worker claims `headroom` on hello and `pulse`; humd hosts no cells |
| 0.7.0 | ensemble plumbing chi (peer-add/remove, attach/detach, wane-sync, gossip-publish, kad-find-node, kad-find-node-resp) |
| 0.6.0 | drone, drone-retrofit, perf-mark first-classed |
| 0.5.0 | release-permit, permission-ask, tendril-reach, tendril-result |
| 0.4.0 | tool-call / tool-result / tool-meta first-classed |
| 0.3.0 | breath / pulse / session-ready handshake refinement |
| 0.2.0 | hello / prompt / chunk / finish baseline |

## See also

- [`thrum-core`](../thrum-core) — Rust source of truth (chi enum, helpers).
- [`thrum-clients/ts`](../thrum-clients/ts) — TypeScript reference client.
- [`thrum-clients/python`](../thrum-clients/python), [`thrum-clients/go`](../thrum-clients/go) — Python + Go reference clients.
- [`thrum-clients/c`](../thrum-clients/c): the C reference client, over libsodium and cJSON.
- [`ensemble/README.md`](../ensemble/README.md) — inter-humd routing.
- [`hives/foragers.md`](../hives/foragers.md) — typology + propensity axes.
- [adiled.github.io/hum](https://adiled.github.io/hum/) — docs site.

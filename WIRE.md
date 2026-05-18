---
title: "thrum wire spec"
description: "the language-neutral protocol nestlings speak to humd. Implementable in any language with an NDJSON parser and a Unix socket library."
---

# thrum wire spec

> _the language-neutral protocol nestlings speak to humd. Implementable in any language with an NDJSON parser and a Unix socket library._

This is the **canonical wire spec**. Rust (`thrum-core`), TypeScript
(`thrum`), Python (`clients/python`), and Go (`clients/go`) are
reference clients — they all conform to the rules below. If a client
disagrees with this document, the document wins.

## Transport

- **Framing**: NDJSON — one UTF-8-encoded JSON object per line,
  separated by `\n` (`0x0A`). No CRLF, no trailing whitespace,
  no leading BOM. Lines that don't parse as JSON are dropped silently
  by humd; clients should do the same.
- **Socket**: Unix stream socket at one of (in priority order)
  - `$HUM_THRUM_SOCK` (explicit override)
  - `$XDG_RUNTIME_DIR/hum/thrum.sock`
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
| `rid` | string | **yes** | Request id. Echoed in correlated responses (e.g. `chi:"echo"`). Format-agnostic; reference clients use `"{base36-ms-timestamp}-{base36-counter}"`. |
| `sid` | string | situational | Session id. Required for `prompt`, `chunk`, `finish`, `tool-call`, etc. Picked by the originator. |
| `from` | string | situational | Sender identity. `HumdId` hex when crossing humds, nestling name when on a local socket. |
| `to` | string | situational | Destination identity. `HumdId` hex for ensemble-routed tones; absent for local-only. |
| `sigil` | string | optional | 12-char content hash, see [Helpers](#helpers). Stable across reconnects for the same (nest, sid). |
| `wane` | integer | optional | Lamport clock per sigil — see [WaneTracker](#wanetracker). |
| `sentAt` | integer | optional | Wall-clock ms at send time. UTC. |
| `dusk` | integer | optional | Absolute ms expiry. If `now > dusk`, the receiver MAY drop. |
| `ext` | object | optional | Per-nestling extension bag. Key it by your nestling name; ignore other keys. |

Beyond the envelope, **each chi defines its own body fields**. A
`chi:"prompt"` tone carries `text`/`content`/`modelId`/`cwd`/etc.
Unknown body fields MUST be preserved on forwarding paths (gRPC
bridge, gossip relay) so future chi extensions are backward-compatible.

## Handshake

The first tone a nestler sends after `connect()` MUST be `chi:"hello"`.
**This tone is the registration**: the humd that receives it now knows
your nestling exists, what chi values you speak, and can route tones
to you. Nothing else is required for a single-machine nestling. The
ensemble layer (peer humds, gossip) and the on-chain layer
(`HumdRegistry`) are additive opt-ins on top.

The handshake doesn't distinguish where the bytes came from. A hello
arriving on humd's local Unix socket and a hello bridged through the
ensemble from a peer humd hit the same code path. Local-dev runs,
local-prod-with-systemd setups, and distributed mesh deployments all
exercise the same `chi:"hello"` shape — see
[nestlings/README.md](../nestlings/) for the three deployment
paradigms (local-dev / local-prod / distributed) that share this
single protocol.

```json
{
  "chi": "hello",
  "rid": "hello-1",
  "from": "<nestling-name>",
  "nestling": "<nestling-name>",
  "version": "<your nestling's semver>",
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

- `nestling`, `protoVersion` — **required**.
- `chis` (note: plural) is the list of chi values this nestling speaks
  and expects to receive. **Distinct from `chi`** at the top level —
  `chi` is the discriminator (this tone IS a hello); `chis` is the
  vocabulary (these are the tones this nestling knows). One word per
  concept; never reuse `chi` for a list.
- All other fields — optional but **strongly recommended**: humd uses
  them to build a `NestlingManifest` and gossip it to the rest of the
  ensemble on the `hum/nestlings/announce` topic.
- humd replies with `chi:"breath"` — a snapshot of any state relevant
  to this nestler (today: `{}`; reserved for future state sync).
- A `protoVersion` mismatch is **a warning, not a hard error**. The
  bumping rules:
  - **patch** — docstring tweaks, additive-optional fields
  - **minor** — new chi value, new required field with compat path
  - **major** — removed chi, renamed chi, semantics changed

## The nest model

A **nest** is the place inside humd where nestlers nestle and **roosts**
(live LLM subprocesses) live. The wire never sees the nest as a thing
of its own — it only sees the chi traffic that flows through it.

Direction is sacred. Two ends of the same connection, two roles:

| who | sends | receives |
|---|---|---|
| **asker side** (a nestler when pre-handshake, a nestled after) | `chi:"hello"` (first ask), then `chi:"prompt"`, `chi:"cancel"`, `chi:"tool-result"`, `chi:"release-permit"`, `chi:"cleanup"`, `chi:"curate"` — keeps asking the whole lifetime | `chi:"breath"` (accepting the handshake), `chi:"chunk"`, `chi:"finish"`, `chi:"tool-call"`, `chi:"permission-ask"`, `chi:"session-ready"`, `chi:"pulse"` |
| **roost side** (compute) | `chi:"chunk"`, `chi:"finish"`, `chi:"tool-call"`, `chi:"permission-ask"` | `chi:"prompt"`, `chi:"tool-result"`, `chi:"release-permit"`, `chi:"cancel"` |

The asker is the same actor throughout — a **nestler** before its
hello is accepted, a **nestled** after. The role doesn't flip when
the state changes; "hello" is just the first of many asks the
connection will carry. Cancels, prompts, tool-results, cleanups all
flow from the nestled state, not just hello from the nestler.

A roost is always answerer. **Nobody is both on the same connection.**
A process that wanted to "also offer compute" would not do so by
inverting its nestler connection — it would do so by being a roost
inside *some humd's nest*, addressable via that humd's ensemble
advertise.

The wire is **opaque to the roost's implementation.** A roost might
be a local subprocess (`claude-cli`), a Rust struct that wraps an
HTTP client to OpenAI's API, a deterministic mock for sim tests —
the wire sees identical chunks coming back either way. The kind of
roost (`WorkerBee` impl, in Rust terms) is the hive's concern,
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
`WorkerBee` impl (any process speaking thrum), point it at a humd, and
the mesh discovers via the existing capability gossip; peers route
prompts your way. The wire stays exactly the same.

## Chi registry (THRUM_VERSION 0.7.0)

Generated by `cargo run -p codegen` — Rust enums in
[`thrum-core/src/chi.rs`](../thrum-core/src/chi.rs) are the source of
truth. The list below is informational and may lag the canonical
registry by one bump.

### Nestler → daemon

| chi | body fields | meaning |
|---|---|---|
| `hello` | `nestling`, `protoVersion`, optional `version`/`propensity`/`chi`/`source` | first frame after connect |
| `prompt` | `sid`, `text`/`content`, optional `modelId`/`cwd`/`systemPrompt`/`tools` | start a turn |
| `cancel` | `sid` | interrupt the current turn for this sid |
| `cleanup` | `sid` | drop daemon state for this session |
| `curate` | `sid` | manual compaction request |
| `release-permit` | `sid`, `permitId`, `decision` | answer a `permission-ask` |
| `tendril-result` | `sid`, `callId`, `result` | task subagent answered |
| `tool-result` | `sid`, `callId`, `result` | nestler-declared tool answered |
| `petal-cell` | `sid`, `cell` | OC message-graph update (graft hint) |

### Daemon → nestler

| chi | body fields | meaning |
|---|---|---|
| `breath` | (state snapshot, usually `{}`) | reply to hello |
| `chunk` | `sid`, `part` (text/reasoning/tool fragment), `index` | streamed model output |
| `finish` | `sid`, `finishReason`, `usage` | turn complete |
| `error` | `sid`, `code`, `message`, optional protocol payload | turn aborted / hard error |
| `session-ready` | `sid`, `claudeSessionId` | nest spawned, ready for prompts |
| `pulse` | `kind` (RoostSpawned/RoostReady/RoostIdle/RoostDied/RoostEvicted), `roostId` | process lifecycle event |
| `permission-ask` | `sid`, `permitId`, `question`, `context` | mid-stream permission needed |
| `tendril-reach` | `sid`, `callId`, `name`, `args` | task subagent dispatch |
| `tool-call` | `sid`, `callId`, `name`, `args` | nestler-declared tool dispatch |
| `tool-meta` | `sid`, `callId`, `meta` | out-of-band metadata for a tool result |

### Either direction

| chi | body fields | meaning |
|---|---|---|
| `echo` | `ok`, optional `error` | delivery ack for `rid` |
| `perf-mark` | `label`, `phase`, `t` | drift timing — measured both ways |
| `log` | `level`, `message`, optional fields | structured log forwarding |
| `drone` | (drone-shaped payload) | drone heartbeat |
| `drone-retrofit` | (retrofit instruction) | drone swallow + retry signal |

### Ensemble / inter-humd plumbing

Only emitted across the ensemble layer (not by local nestlers):

`peer-add`, `peer-remove`, `attach`, `detach`, `wane-sync`,
`gossip-publish`, `kad-find-node`, `kad-find-node-resp`.
See [`ensemble/README.md`](../ensemble/README.md).

## Helpers

The reference clients ship three deterministic helpers. Algorithms
are pinned by the protocol — any client implementation MUST match
byte-for-byte.

### `sigil(sid, nest)`

```
sigil = lowercase_hex(sha256(nest + ":" + sid)[..6])      // 12 chars
```

Stable identifier for a `(nest, session)` pair. Used by humd's
WaneTracker keys and by drift detection. `nest` is the nest-kind
namespace ("claude-cli", "claude-repl", future kinds); `sid` is the
session id the nestler chose. No fallback — `nest` is required.

### `rid()`

```
rid = base36(now_ms) + "-" + base36(counter++)
```

Monotonic correlation id. Counter is per-process and starts at 0.
Format-agnostic on receive — only the originator's correlation logic
cares about the exact format.

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

A nestler-declared tool is an arbitrary callable the nestling exposes
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

1. Resolve the socket path (env override → XDG runtime → `/run/user/<uid>`).
2. `connect()` over a Unix stream socket. Buffer incoming bytes,
   split on `\n`, JSON-parse each line, drop on parse error.
3. Send `chi:"hello"` immediately after connect. Buffer subsequent
   writes until connect resolves.
4. Multiplex by `sid`: keep a `Map<sid, handler>` plus one catch-all
   handler for tones without a sid (breath, echo, pulse).
5. Serialize outbound tones with newline termination (`json.dumps(t) + "\n"`).
6. On close: drop any pending writes, surface the disconnect to handlers.

Reference: [`thrum-core`](../thrum-core), [`thrum`](.),
[`clients/python`](../clients/python),
[`clients/go`](../clients/go).

## Version history

| THRUM_VERSION | what changed |
|---|---|
| 0.7.0 | ensemble plumbing chi (peer-add/remove, attach/detach, wane-sync, gossip-publish, kad-find-node, kad-find-node-resp) |
| 0.6.0 | drone, drone-retrofit, perf-mark first-classed |
| 0.5.0 | release-permit, permission-ask, tendril-reach, tendril-result |
| 0.4.0 | tool-call / tool-result / tool-meta seam |
| 0.3.0 | breath / pulse / session-ready handshake refinement |
| 0.2.0 | hello / prompt / chunk / finish baseline |

## See also

- [`thrum-core`](../thrum-core) — Rust source of truth (chi enum, helpers).
- [`thrum`](./) — TypeScript reference client.
- [`clients/python`](../clients/python), [`clients/go`](../clients/go) — Python + Go reference clients.
- [`ensemble/README.md`](../ensemble/README.md) — inter-humd routing.
- [`nestlings/README.md`](../nestlings/README.md) — typology + propensity axes.
- [adiled.github.io/hum](https://adiled.github.io/hum/) — docs site.

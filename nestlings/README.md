# nestlings

> _what a nestler conforms to, for becoming nestled_

A **nestling** is the *kind* — a representation that knows how to nestle
into a hum. A running instance of one is a **nestler**. After the
handshake, it's **nestled**. The directory you're in is the catalogue.

Each nestling sits between some external world and the hum daemon (humd),
speaking thrum on the inside and whatever its consumer expects on the
outside. The nestling decides which parts of thrum it cares about and
hides the rest behind a contract its outside world already speaks.

## Propensities

Three orthogonal axes shape every nestling.

### 1. Statefulness — how much hum-state does it carry

| level | meaning |
|---|---|
| **stateful** | holds its own session graph, agent identity, tools, MCP configs; pushes updates back over thrum (`petal-cell`, `cleanup`) |
| **convention-stateful** | no server-side state, but the *protocol it speaks* implies continuation (e.g. OpenAI's `user` field) |
| **stateless per-call** | each call is a fresh hum sid; history flows only through what the consumer re-supplies |
| **transport-only** | bytes in, bytes out; state is the client's problem |

### 2. Richness — how much context it can hand to humd

| level | hands humd | gets in return |
|---|---|---|
| **rich** | cwd, permissions, agent, MCP configs, prior petals, plan mode | full hum experience — graft, tool brokering, drone |
| **medium** | tools, system, content | inference + nestler-declared tools |
| **lean** | content, system | inference only; pure model I/O |
| **opaque** | passthrough JSON | whatever the client puts on the wire |

### 3. Wire shape — what the outside world calls it

The nestling picks a contract that already exists in the wild, then
translates. The shape determines which chi values survive the translation
and which are dropped.

## Current catalogue

| nestling | statefulness | richness | wire shape | hides |
|---|---|---|---|---|
| **opencode** | stateful | rich | OC plugin (events + provider + tools) | almost nothing — speaks every chi |
| **openai-server** | convention-stateful | medium | OpenAI `/v1/chat/completions` SSE | pulse, breath, drone, perf-mark, tendril, permission-ask, tool-meta |
| **vercel-ai** | stateless per-call | lean | Vercel AI SDK `LanguageModelV3` | same as openai-server + session-ready, log, echo |
| **grpc** | transport-only | opaque | bidi `Stream(stream Tone)` RPC | nothing — every chi flows through |

## The chi cheatsheet

`thrum/chi.ts` owns the full registry. Common subsets a new nestling
might pick:

| subset | chi values |
|---|---|
| inference-only | `prompt`, `chunk`, `finish`, `error`, `cancel` |
| + tools | `tool-call`, `tool-result` |
| + permissions | `permission-ask`, `release-permit` |
| + state sync | `hello`, `breath`, `session-ready`, `pulse`, `cleanup` |
| + observability | `perf-mark`, `drone`, `drone-retrofit`, `echo`, `log` |
| + everything | `tendril-reach`, `tendril-result`, `tool-meta`, `petal-cell`, `curate` |

Lean nestlings deliberately drop tones they can't express. Rich ones
forward everything and lean on consumer-side machinery to make sense
of it.

## Adding a new nestling

1. **Pick a propensity** on each axis. The dimensions decide which
   chi values you can keep.
2. **Add a thrum client** — see any existing `src/thrum.ts`; ~90 LoC.
   Connect, send NDJSON, dispatch incoming tones to per-sid handlers.
3. **Announce yourself** — emit `chi:"hello"` on connect with
   `nestling`, `protoVersion`, and your own `version`. Daemon traces
   the announcement and warns on version mismatch.
4. **Translate or transport.** Translators map thrum chunks into the
   outside contract's shape (see `vercel-ai/src/transform.ts`).
   Transports just forward bytes (see `grpc/src/index.ts`).
5. **Tools, if you have them.** Forward incoming `chi:"tool-call"` to
   whatever your outside world considers a tool. Ship the answer back
   as `chi:"tool-result"` with the same `callId`. The daemon will
   resume the parked model.

## Versioning

`THRUM_VERSION` lives in `thrum/chi.ts` and is independent of any
package version. Bump rules:

- **patch**: docstring tweaks, additive-optional fields
- **minor**: new chi value, new required field with backward-compat path
- **major**: removed chi, renamed chi, semantics changed

Each nestling pins the version it targets in its own `src/thrum.ts`.
Daemon traces every mismatch.

## Future propensities

- **observer** — hear-only; attach to an existing hum, drive nothing.
  Useful for dashboards, audit, replay.
- **scaffold** — language-grade tooling that maps `chi:"tool-call"` to
  LSP-style commands.
- **mesh** — nestler-to-nestler; one hum's output re-broadcast to peers.

Each is a recipe in waiting. Compose the right combination of statefulness
× richness × wire-shape and you've got a new nestling.

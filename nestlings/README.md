---
title: "nestlings"
description: "what a nestler conforms to, for becoming nestled"
---

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
| **anthropic-server** | convention-stateful | medium | Anthropic `/v1/messages` SSE | pulse, breath, drone, perf-mark, tendril, permission-ask, tool-meta |
| **vercel-ai** | stateless per-call | lean | Vercel AI SDK `LanguageModelV3` | same as openai-server + session-ready, log, echo |
| **ollama-server** | convention-stateful | medium | Ollama `/api/chat` + `/api/generate` NDJSON | pulse, breath, drone, perf-mark, tendril, permission-ask, tool-meta |
| **paid-oracle** | stateless per-call | lean | x402-over-tool-call | everything except hello/tool-call/tool-result/error |
| **grpc** | transport-only | opaque | gRPC bidi `Stream(stream Tone)` | nothing — every chi flows through |
| **twilio-sms** | stateful (per-phone sid) | lean | Twilio Messaging webhook | tools, system, perf, drone, breath |
| **gsm-modem** | stateful (per-phone sid) | lean | GSM AT-command serial | tools, system, perf, drone, breath |

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

The `nestlings/` directory in this repo is a catalogue of reference
implementations. **Adding a new nestling has nothing to do with this
directory.** A nestling is just a process that handshakes with a
humd over thrum. The handshake itself **is** the registration —
humd now knows your nestling exists, what chi values it speaks, and
can route tones to it.

What happens beyond that depends on which humd features you've
turned on. The three layers stack additively:

| layer | when it matters | who learns about your nestling |
|---|---|---|
| **local humd (thrum)** | always — happens on every `chi:"hello"` | the humd you handshook with (and any other nestler connected to that same humd) |
| **ensemble (mesh)** | only when the humd is wired to peer humds | every peer humd that subscribes to `hum/nestlings/announce` |
| **on-chain** | only when you advertise to a `HumdRegistry` deployment | anyone reading that registry on whatever chain it lives on |

You get the first for free by handshaking. The other two are
opt-ins, useful only when you want distributed discovery or
censorship-resistant identity.

### Steps

1. **Pick a propensity** on each axis. The dimensions decide which
   chi values you can keep.
2. **Import the contract.**
   - Rust: `cargo add thrum-core` (today: `{ git = "https://github.com/adiled/hum.git" }`)
   - TS: `npm install thrum` (today: `"thrum": "git+https://github.com/adiled/hum.git#main"`)
   - Python: `pip install thrum` (today: `pip install "git+https://github.com/adiled/hum.git#subdirectory=clients/python"`)
   - Go: `go get github.com/adiled/hum/clients/go/thrum`
3. **Add a thrum client** — see any existing `src/thrum.ts`; ~90 LoC.
   Connect to the humd socket at `$XDG_RUNTIME_DIR/hum/thrum.sock`,
   send NDJSON, dispatch incoming tones to per-sid handlers.
4. **Handshake** — emit `chi:"hello"` on connect with `nestling`,
   `protoVersion`, your own `version`, and optionally `propensity`,
   `chis` (the array of chi values you speak), and `source` (URL to
   your nestling's repo). Humd records you locally as soon as this
   tone lands. **You are now registered with that humd.** Nothing
   else is required for solo / single-machine use.
5. **Translate or transport.** Translators map thrum chunks into the
   outside contract's shape (see `vercel-ai/src/transform.ts`).
   Transports just forward bytes (see `grpc/src/index.ts`).
6. **Tools, if you have them.** Forward incoming `chi:"tool-call"` to
   whatever your outside world considers a tool. Ship the answer back
   as `chi:"tool-result"` with the same `callId`. The daemon will
   resume the parked model.

That's the whole baseline. Single-machine nestlings stop here.

### Want peer humds to discover you?

Turn on ensemble in the humd that hosts you (it's already on by
default in shipped configs; nothing to do per-nestling). humd then
gossips a `NestlingManifest` built from your hello on
`hum/nestlings/announce`. Any humd subscribed to that topic learns:

```rust
use ensemble::Ensemble;
let mut found = ensemble.nestling_discover("market-maker");
while let Some((humd_id, manifest)) = found.recv().await {
    // dial them, transact, settle
}
```

See [`ensemble/README.md`](../ensemble/README.md) for the full
discover path including Kademlia lookup of unknown HumdAddrs.

### Want on-chain identity?

For humds that want a censorship-resistant alternative to gossip,
deploy your own `HumdRegistry` and publish the manifest hash there —
see [`contracts/`](../contracts/). This is independent of ensemble:
a solo humd can publish to its own on-chain registry without ever
meshing.

### Want your nestling listed in this repo as reference?

Listing in **this** repo's catalogue table is editorial — for
nestlings the maintainers consider exemplars. A PR is optional and
unrelated to registration; your nestling is registered with humd
regardless.

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

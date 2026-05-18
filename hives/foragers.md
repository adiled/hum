---
title: "bees"
description: "what a nestler conforms to, for becoming nestled"
---

# bees

> _what a nestler conforms to, for becoming nestled_

A **bee** is the *kind* — a representation that knows how to nestle
into a hum. A running instance of one is a **nestler**. After the
handshake, it's **nestled**. The directory you're in is the catalogue.

Each bee sits between some external world and the hum daemon (humd),
speaking thrum on the inside and whatever its consumer expects on the
outside. The bee decides which parts of thrum it cares about and
hides the rest behind a contract its outside world already speaks.

## Propensities

Three orthogonal axes shape every bee.

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

The bee picks a contract that already exists in the wild, then
translates. The shape determines which chi values survive the translation
and which are dropped.

## Current catalogue

| bee | statefulness | richness | wire shape | hides |
|---|---|---|---|---|
| **openai-server** | convention-stateful | medium | OpenAI `/v1/chat/completions` SSE | pulse, breath, drone, perf-mark, tendril, permission-ask, tool-meta |
| **anthropic-server** | convention-stateful | medium | Anthropic `/v1/messages` SSE | pulse, breath, drone, perf-mark, tendril, permission-ask, tool-meta |
| **vercel-ai** | stateless per-call | lean | Vercel AI SDK `LanguageModelV3` | same as openai-server + session-ready, log, echo |
| **ollama-server** | convention-stateful | medium | Ollama `/api/chat` + `/api/generate` NDJSON | pulse, breath, drone, perf-mark, tendril, permission-ask, tool-meta |
| **paid-oracle** | stateless per-call | lean | x402-over-tool-call | everything except hello/tool-call/tool-result/error |
| **grpc** | transport-only | opaque | gRPC bidi `Stream(stream Tone)` | nothing — every chi flows through |
| **twilio-sms** | stateful (per-phone sid) | lean | Twilio Messaging webhook | tools, system, perf, drone, breath |
| **gsm-modem** | stateful (per-phone sid) | lean | GSM AT-command serial | tools, system, perf, drone, breath |

## The chi cheatsheet

`clients/ts/chi.ts` owns the full registry. Common subsets a new bee
might pick:

| subset | chi values |
|---|---|
| inference-only | `prompt`, `chunk`, `finish`, `error`, `cancel` |
| + tools | `tool-call`, `tool-result` |
| + permissions | `permission-ask`, `release-permit` |
| + state sync | `hello`, `breath`, `session-ready`, `pulse`, `cleanup` |
| + observability | `perf-mark`, `drone`, `drone-retrofit`, `echo`, `log` |
| + everything | `tendril-reach`, `tendril-result`, `tool-meta`, `petal-cell`, `curate` |

Lean bees deliberately drop tones they can't express. Rich ones
forward everything and lean on consumer-side machinery to make sense
of it.

## Three paradigms for running a nestler

The protocol doesn't care where the nestler runs. The same `chi:"hello"`,
manifest, prompt/chunk/finish triad works in every shape. What differs is
*deployment*. Three honest modes:

### Paradigm 0 — local dev (no install, no systemd, no ensemble)

The minimal case. Used by contributors and by anyone running hum from
a clone.

```bash
# terminal 1 — humd binds $XDG_RUNTIME_DIR/hum/thrum.sock
cargo run -p humd

# terminal 2 — the nestler launches itself; reads the same default path
cd hives/openai-server
pnpm install && pnpm run build
node dist/index.js

# terminal 3 — talk to humd's nest via the openai-shape door
curl http://127.0.0.1:14620/v1/models
```

No install scripts. No persistence across reboot. No multi-humd story.
The socket location is the only contract — both sides resolve it via
the canonical XDG path (or `HUM_THRUM_SOCK` override).

This paradigm is what every clone-and-build path should land on.

### Paradigm 1 — ensemble (mesh wire, distributed thrums)

**The practical sweet spot.** A nestler on machine X reaches a humd
on machine Y over the ensemble — its `chi:"hello"` lands at humd-Y
through the transport (TCP / TLS / Iroh) instead of a local socket.

From humd-Y's point of view: nothing new. Same hello, same manifest,
same routing. The transport layer bridges bytes; the protocol layer
doesn't know or care.

What enables this:
- humd has ensemble built in — wakes the moment `peers.json` has at
  least one bootstrap entry
- the remote nestler runs by paradigm 0 (or 2) on *its own* machine
- humd-Y advertises its nests via `PeerCapabilities.nests`; peer
  humds discover and route prompts via the ensemble's `route(tone)`

**Nothing extra installs on the humd side.** A foreign nestler doesn't
touch humd-Y's filesystem. The `source` URL in its hello tells the
mesh what kind of nestler it is and where its repo lives — purely
informational; no code is downloaded.

Why this is the everyday mode for any non-trivial deployment:
compute pools naturally across machines, one humd's models become
shared compute for many nestlers without you provisioning systemd on
each.

### Paradigm 2 — managed bee service provisioning (advanced)

You're running a hum-on-this-machine fleet — persistent across
reboots, with per-bee systemd units, per-kind configs on disk,
the whole operational kit.

```bash
./install                                  # humd: binary + identity + systemd unit
hives/openai-server/install            # one per bee you want hosted
hives/anthropic-server/install         # etc.
```

What this adds over paradigm 0:
- humd binary at `~/.local/bin/humd`
- Ed25519 identity at `~/.local/state/hum/humd.key`
- `~/.config/hum/hum.json` seeded with the namespaced 0.3 shape
- `~/.config/hum/hives/<kind>.json` per bee kind
- systemd user units (`hum.service`, `hum-openai-server.service`, …)
- `~/.config/hum/peers.json` scaffolded empty

This is the heaviest mode. Worth it when you want each bee
running as a managed service on this host; otherwise paradigm 1
gives you most of the value with much less ops.

### Quick which-paradigm-am-I-using

| signal | likely paradigm |
|---|---|
| no systemd unit, terminal-launched | 0 |
| `peers.json` has entries and humd's logs show inbound `hello` from non-local addrs | 1 |
| `~/.config/hum/hum.json` exists, multiple `hum-*.service` units registered | 2 |

The protocol is identical across all three. Install scripts are
paradigm-1 ergonomics, not protocol requirements.

## Adding a new bee

The `hives/` directory in this repo is a catalogue of reference
implementations. **Adding a new bee has nothing to do with this
directory.** A bee is just a process that handshakes with a
humd over thrum. The handshake itself **is** the registration —
humd now knows your bee exists, what chi values it speaks, and
can route tones to it.

What happens beyond that depends on which humd features you've
turned on. The three layers stack additively:

| layer | when it matters | who learns about your bee |
|---|---|---|
| **local humd (thrum)** | always — happens on every `chi:"hello"` | the humd you handshook with (and any other nestler connected to that same humd) |
| **ensemble (mesh)** | only when the humd is wired to peer humds | every peer humd that subscribes to `hum/hives/announce` |
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
4. **Handshake** — emit `chi:"hello"` on connect with `bee`,
   `protoVersion`, your own `version`, plus `source` (URL to your
   bee's repo), `bind` (the host/port you're listening on, if
   you bind one), `propensity`, and `chis` (the chi values you speak).
   The `source` is non-trivial: humd records it in the manifest so
   peer humds on the mesh can find the repo and install the same
   bee kind locally. You are now registered with humd. Nothing
   else is required for solo / single-machine use.
5. **Translate or transport.** Translators map thrum chunks into the
   outside contract's shape (see `vercel-ai/src/transform.ts`).
   Transports just forward bytes (see `grpc/src/index.ts`).
6. **Tools, if you have them.** Forward incoming `chi:"tool-call"` to
   whatever your outside world considers a tool. Ship the answer back
   as `chi:"tool-result"` with the same `callId`. The daemon will
   resume the parked model.

That's the whole baseline. Single-machine bees stop here.

### Want peer humds to discover you?

Turn on ensemble in the humd that hosts you (it's already on by
default in shipped configs; nothing to do per-bee). humd then
gossips a `NestlingManifest` built from your hello on
`hum/hives/announce`. Any humd subscribed to that topic learns:

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

### Want your bee listed in this repo as reference?

Listing in **this** repo's catalogue table is editorial — for
bees the maintainers consider exemplars. A PR is optional and
unrelated to registration; your bee is registered with humd
regardless.

## Versioning

`THRUM_VERSION` lives in `clients/ts/chi.ts` and is independent of any
package version. Bump rules:

- **patch**: docstring tweaks, additive-optional fields
- **minor**: new chi value, new required field with backward-compat path
- **major**: removed chi, renamed chi, semantics changed

Each bee pins the version it targets in its own `src/thrum.ts`.
Daemon traces every mismatch.

## Future propensities

- **observer** — hear-only; attach to an existing hum, drive nothing.
  Useful for dashboards, audit, replay.
- **scaffold** — language-grade tooling that maps `chi:"tool-call"` to
  LSP-style commands.
- **mesh** — nestler-to-nestler; one hum's output re-broadcast to peers.

Each is a recipe in waiting. Compose the right combination of statefulness
× richness × wire-shape and you've got a new bee.

# humd-rs

> _the daemon, recast in a language that doesn't pause to breathe_

A Rust spike of humd — same wire, same vocabulary, different floor.
`humd-rs` is intended as a drop-in for the TypeScript daemon: the
nestlings on the outside shouldn't be able to tell which one is humming.

This is a **spike**. Some crates are dense; some are a single line.
The carve is fixed; the filling is in flight.

## Why

The TypeScript humd does its job, but four pressures pull toward Rust:

- **PTY sturdiness.** The Claude CLI roost is a hostile process under a
  pseudo-terminal — ANSI noise, mid-frame redraws, half-broken UTF-8
  during teardown. A statically-typed FSM with byte-level control reads
  that stream more honestly than an event-loop coroutine ever will.
- **GC-free streaming.** Tones land tens of times per second per hum.
  Chunk deltas land hundreds. A daemon that yields to a collector mid-bloom
  shows up as a stutter on every nestler at once. Rust just doesn't.
- **Cold-start.** humd should feel like it was already there when the
  first nestler knocks. A native binary opens its sockets before Node
  has parsed its first import.
- **Predictable memory.** A long-lived daemon holding hundreds of
  hums, drift rings, and drones earns its right to a flat RSS curve.

None of these are urgencies. They're aspirations — the shape we want
humd to settle into once the protocol stops moving.

## The carve

Each crate owns one concern. The vocabulary tracks the TypeScript side
exactly; only the implementation changes.

| crate | concern |
|---|---|
| **thrum-core** | wire types, envelope, chi enum, sigil/wane primitives |
| **thrumd** | the NDJSON unix-socket server — listen, dispatch, broadcast |
| **nest** | the Claude subprocess pool — two perches: pipe + PTY |
| **mcpd** | hum's MCP server — HTTP JSON-RPC, native tools, passthrough |
| **graft** | JSONL stitching for Claude transcripts — graft, sanitize, prune |
| **drift** | per-hum timing rings — marks, spans, thrum samples, p50/p95 |
| **drone** | the sentinel — observes tones, classifies suspicion, assesses |
| **hums** | hum-state registry — atomic JSON load/save under XDG_STATE_HOME |
| **penny** | lifetime counters — increment-only, merge-friendly |
| **config** | `hum.json` loader — XDG-located, defaults fill missing fields |
| **ids** | 256-bit Crockford-base32 ids — 48-bit ms + 208 random |
| **codegen** | mirror `thrum/chi.ts` into the Rust chi enum |
| **humd-bin** | the executable — wires the crates together |

## What's ported, what's redesigned

The ports preserve behavior verbatim. The redesigns are the parts where
the TypeScript shape was load-bearing on a runtime humd-rs doesn't have.

**Rigorously ported.** The wire, the contracts, the semantics every
nestling already relies on:

- **thrumd** — same socket path, same NDJSON framing, same dispatch
  rules. A nestler connected to TS humd should be able to swap to
  Rust humd and not notice.
- **mcpd** — same HTTP surface, same native tool set, same passthrough
  to nestler-declared and external MCP tools.
- **petal-cell semantics** — the daemon's view of the nestler's
  conversation graph is preserved tone-for-tone. Graft, prune, retag
  all behave identically.
- **chi enum** — generated from `thrum/chi.ts` so the two
  implementations cannot drift on what counts as a known tone.

**Redesigned.** Where the TS implementation paid a tax for a runtime
Rust doesn't have:

- **PTY classifier.** The TS daemon scrubs CLI output with regex
  passes; Rust does it as a byte-level FSM with explicit states for
  ANSI escapes, prompt-frame transitions, and partial UTF-8. Faster,
  but more importantly: deterministic.
- **Harness FSM.** The roost's lifecycle (spawning, ready, idle,
  evicted, dead) is one explicit state machine, not a Promise graph.
  Pulse tones fall out of state transitions instead of being emitted
  by hand.

## The wire contract

humd-rs binds the same socket as TS humd and speaks the same tones.
A nestler connects, sends `hello`, gets `breath`, prompts, receives
`chunk` and `finish`. No nestling change. No flag flip on the consumer.
The contract is `thrum/chi.ts` and the two daemons share it.

The MCP side is identical: the same HTTP server bound to the same port,
serving the same native tools with the same names, schemas, and
semantics. External MCP servers are reached the same way; nestler-declared
tools are brokered back over thrum the same way.

## What stays in TypeScript

The nestlings stay where they are. They're already small (~90 LoC of
thrum client per nestling), already correct, and already shaped by the
outside contracts they translate to. Rewriting them buys nothing.

- **opencode** — plugin into OpenCode's TS plugin system
- **vercel-ai** — implements `LanguageModelV3` from the AI SDK
- **openai-server** — serves OpenAI's `/v1/chat/completions` SSE
- **grpc** — bidi `Stream(stream Tone)` RPC, transport-only

Each pins its `THRUM_VERSION` and connects to whichever humd is bound
to the socket. The daemon is the seam; the nestlings are the surface.

## The chi-sync story

`thrum/chi.ts` is the canonical registry. Touching the wire means
touching that file and bumping `THRUM_VERSION`.

`humd-rs/codegen` is a tiny CLI: it reads `thrum/chi.ts`, parses the
`Chi` object and the `THRUM_VERSION` literal, and emits
`thrum-core/src/chi.rs`. The TS file is the source of truth; the Rust
enum is downstream. Drift between them is mechanically impossible — if
the regen wasn't run, the build fails on a chi nobody knows about.

Run `cargo run -p codegen` whenever `thrum/chi.ts` moves.

## Status

A spike, honestly named.

| crate | status |
|---|---|
| thrum-core | drafted — envelope, chi, sigil, wane |
| ids | done — 256-bit ids match TS |
| codegen | drafted — parses chi.ts, emits Rust enum |
| config | drafted — XDG loader |
| nest | drafted — pipe + PTY perches both stubbed in |
| mcpd | drafted — protocol + session sketched |
| thrumd | drafted — listener + registry sketched |
| graft, drift, drone, hums, penny | placeholder `lib.rs` |
| humd-bin | placeholder `lib.rs` |

The workspace currently builds only `thrum-core` and `ids`; the rest
are carved but not yet wired into `[workspace.members]`. Expanding the
members list is the first checkpoint.

## Build + run

```bash
cd /root/clwnd/humd-rs
cargo build --workspace        # build everything wired in
cargo test  --workspace        # run what tests exist
cargo run   -p codegen         # regenerate chi.rs from thrum/chi.ts
```

There is no `cargo run -p humd-bin` yet — the binary is a placeholder.

## Migration plan

Parallel-run. Don't cut over until the new daemon has hummed for real.

1. **Bind on an alt socket.** humd-rs starts under `HUM_SOCKET` pointing
   at a sibling path. TS humd keeps the canonical socket.
2. **One nestling at a time.** Point a single nestler at the alt socket
   via the same env var. Compare drift rings, drone assessments, and
   transcripts side-by-side.
3. **Promote when quiet.** When humd-rs runs a real workload for a real
   day without a single drone retrofit and a flat drift curve, swap the
   socket paths. TS humd becomes the alt; humd-rs becomes canonical.
4. **Retire when ignored.** When nobody's needed the TS fallback for a
   week, the directory comes out. Until then it stays — the wire
   contract is shared, so falling back is a `HUM_SOCKET` away.

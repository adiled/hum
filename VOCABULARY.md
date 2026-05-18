---
title: "Vocabulary"
description: "The biodiverse register hum thinks in. Names carry meaning beyond function."
---

# Vocabulary

This page is a quick lookup of every word that appears in the hum
codebase as a load-bearing term. Names are chosen so they *feel*
like the thing — readers and writers share the same mental model.

## Wire

- **thrum** — the bidirectional NDJSON socket between humd and any nestler.
- **tone** — one message frame on the thrum. Envelope (chi/rid/sid/…) plus body.
- **chi** — the tone's discriminator. `prompt`, `chunk`, `finish`, `gossip-publish`, …
- **chis** — the *list* of chi values a nestling speaks. Advertised in the hello.
- **sigil** — content-addressable session pairing hash. Stable across reconnects.
- **wane** — Lamport-clock per sigil. Increments on every state mutation.
- **dusk** — absolute ms expiry on a tone. Past dusk, drop.

## Daemon

- **humd** — the daemon process. One per machine install.
- **HumdId** — sha256 of the humd's Ed25519 public key.
- **hum** — one conversation. Has a hum_id, lives on a humd.
- **nest** — the *place* inside humd where bees gather and roosts live.
  Not a process. Not a model. The meeting space. One humd, one nest.
- **roost** — one live LLM subprocess living in the nest. The compute itself —
  what turns a `chi:"prompt"` into `chi:"chunk"` + `chi:"finish"`. Fungible:
  spawn, kill, respawn.
- **brood** — the state machine that walks a roost from cold to ready (PTY-only).

## Conversation

- **petal** — one unit of content (text, image, tool_use, tool_result).
- **petal-cell** — a nestler's view of one petal in its own conversation graph.
- **bloom** — one turn of conversation. Starts with a prompt, ends with a finish.
- **wilt** — close the bloom.
- **buds** — buffered tool events not yet committed.
- **shed** — flush the buds.
- **tendril** — brokered tool call. Reaches across the wire for execution.
- **sap** — accumulated tool input being assembled.

## Ensemble

- **ensemble** — the mesh of cooperating humds.
- **hive** — the *kind*. A typology slot. Doesn't run; doesn't send anything.
  The conformance an actor must match to be allowed to commission bees.
  `claude-cli` is a hive; `openai-server` is a hive; `market-maker` is a hive.
- **bee** — the *instance*. A running participant commissioned by a hive.
  Each bee declares its kinds on `chi:"hello"` via `bee: ["worker"]`,
  `bee: ["forager"]`, or both (hybrid bees are allowed).
  - **worker** — produces compute. Implements the `WorkerBee` trait
    (or its non-Rust equivalent) — accepts `chi:"prompt"` for advertised
    models and emits `chi:"chunk"` / `chi:"finish"` / `chi:"tool-call"`.
  - **forager** — translates outside wire ↔ thrum. Implements
    `ForagerBee` — wraps an HTTP / gRPC / stdio surface into thrum tones.
- **nestler** — the bee *in the act of joining* the nest. The live process
  sending its first ask: `chi:"hello"`. Awaits the breath that confirms
  handshake. (Verb-state, not a kind.)
- **nestled** — the bee *after joining* the nest. Same actor, registered.
  Has a nestledId. **Keeps asking throughout the connection** —
  `chi:"prompt"`, `chi:"cancel"`, `chi:"tool-result"`,
  `chi:"release-permit"`, `chi:"cleanup"`, `chi:"curate"`. Hello is the
  first ask; nestled is what asks the rest.

The four are not synonyms. Hive describes the kind; bee is the running
participant; nestler arrives; nestled inhabits. The transition nestler →
nestled is one of *state*, not function: the actor is an asker through
both. A nestled bee shares the nest with the roosts that live there, and
continues sending chi at it until disconnect.

## Observation

- **drone** — the sentinel watching every tone. Self-governance + drift detection.
- **drift** — timing rings per bloom. p50/p95 across humds.
- **penny** — lifetime counters. Token swaps, tool executions, etc.

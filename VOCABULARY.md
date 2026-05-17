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
- **nest** — the *place* inside humd where nestlers nestle and roosts live.
  Not a process. Not a model. The meeting space. One humd, one nest.
- **roost** — one live LLM subprocess living in the nest. The compute itself —
  what turns a `chi:"prompt"` into `chi:"chunk"` + `chi:"finish"`. Fungible:
  spawn, kill, respawn.
- **Perch** — the Rust trait. Defines a *kind* of roost
  (`claude-cli`, `claude-repl`, future `openai-api`, future `ollama-local`).
  A Perch impl owns its roost's lifecycle. Loaded into humd at build time.
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
- **nestling** — the *kind*. A typology slot. Doesn't run; doesn't send anything.
  The conformance an actor must match to be allowed to nestle. `openai-server`
  is a nestling; `market-maker` is a nestling.
- **nestler** — the *instance*. The live process. Sends `chi:"hello"`,
  nestles into a humd. Always the asker direction: produces `chi:"prompt"`,
  consumes `chi:"chunk"`/`chi:"finish"`. One running OC plugin = one nestler.
- **nestled** — the *state*. What a nestler is called once its handshake has
  been accepted and its connection is registered. A nestled has a nestledId.

The three are not synonyms. Nestling describes; nestler runs; nestled is the
condition after acceptance. A nestler nestles into a humd's nest. Once
nestled, it shares that nest with the roosts that live there.

## Observation

- **drone** — the sentinel watching every tone. Self-governance + drift detection.
- **drift** — timing rings per bloom. p50/p95 across humds.
- **penny** — lifetime counters. Token swaps, tool executions, etc.

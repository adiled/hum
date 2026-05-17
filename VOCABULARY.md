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
- **nest** — a class of model harness (claude-cli, claude-repl, future kinds).
- **roost** — one live nest process (one Claude subprocess, say).
- **perch** — the strategy that spawns a roost.
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
- **nestling** — the kind a nestler conforms to. The OC plugin is one; the
  market-maker agent is another.
- **nestler** — one running instance of a nestling.
- **nestled** — a nestler post-handshake. After it has nestled.

## Observation

- **drone** — the sentinel watching every tone. Self-governance + drift detection.
- **drift** — timing rings per bloom. p50/p95 across humds.
- **penny** — lifetime counters. Token swaps, tool executions, etc.

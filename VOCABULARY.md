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
- **chi** — the tone's discriminator. `prompt`, `unfurl`, `wilt`, `gossip-publish`, …
- **chis** — the *list* of chi values a bee speaks. Advertised in the hello.
- **sigil** — what ties a run of blooms together: a hash of their `sid`. Stable across reconnects.
- **wane** — Lamport-clock per sigil. Increments on every state mutation.
- **dusk** — absolute ms expiry on a tone. Past dusk, drop.

## Daemon

- **humd**: the daemon process. One per machine install. A coordination
  node: it admits bees, routes between them and gossips with its peers.
  It hosts no cells.
- **HumdId**: sha256 of the humd's Ed25519 public key.
- **nest**: the cells a hive hosts and the limits it keeps them under. A
  hive turns it on by using hum-nest, and the limits (`maxCells`,
  `idleMs`, cell `bounds`) are its own, in the `nest` section of its
  `hives/<hive>.json`. A humd hosts no nest.
- **cell**: one live LLM subprocess in a hive's nest. The compute itself,
  what grows a `chi:"prompt"` into `chi:"unfurl"`s and a `chi:"wilt"`.
  Fungible: spawn, kill, respawn.
- **headroom**: how many more cells a worker can take, which it claims on
  its hello and on each cell pulse. humd sends a prompt to a worker with
  room, here or on a peer.
- **brood**: the state machine that walks a cell from cold to ready (PTY-only).

## Conversation

- **petal** — one unit of content. Its kind says which: **text**, a
  **tendril**, the **grasp** a tendril came back with, **pollen**, a
  **sepal**, a **waggle**, a **thorn**, **brix**, a **bouquet**.
- **petal-cell** — a nestler's view of one petal in its own conversation graph.
- **bloom** — one turn of conversation. Opens with a `prompt`, closes with a `wilt`.
- **unfurl** — a petal growing, one tone at a time, until it is closed.
- **wilt** — close the bloom. It carries the bloom's nectar.
- **buds** — buffered tool events not yet committed.
- **shed** — flush the buds.
- **tendril** — brokered tool call. Reaches across the wire for execution
  (`tendril-reach`), and comes back with what it grasped (`tendril-result`).
- **sap** — accumulated tool input being assembled; it grows in an unfurl.
- **pollen** — media carried as a petal: one **grain** of image, audio or file.
- **sepal** — the cell's reasoning, wrapped around the bloom before it opens.
- **propolis** — the seal on a sepal, so the reasoning carries into the next bloom.
- **waggle** — where a petal's words came from, as a dance points to a source.
- **thorn** — the cell declining, or a filter acting.
- **brix** — how likely each token was, as sugar is read in nectar.
- **bouquet** — several candidate petals gathered for one prompt.
- **scout** — a subagent sent out to explore, which comes back with what it found.

How a bloom grows is said on its prompt: its **temperature**, **prune**
(cut the unlikely tail), **season** (how long it may grow), **frost**
(where it stops), **seed**, **trellis** (the shape it grows along),
**tendrils** (which may reach) and **sepal** (how much it reasons first).

## Ensemble

- **ensemble**: the mesh of cooperating humds.
- **hive**: the *kind*. A directory with an `Orchfile` is a hive, and
  that is the whole test; what it takes to be a bee of it is the hive's
  own business. A hive doesn't run and doesn't send anything.
  `claude-cli` is a hive; `openai-server` is a hive; `market-maker` is a hive.
- **hive installation**: teaching a humd a kind (`hum hive <ref> install`).
  It records the hive on that humd and runs the hive's `hive-install`
  unit, if it has one; everything else the hive runs is its own business.
  A humd understands a bee only through a hive installed there, and never
  routes to a remote bee of a kind it was never taught.
- **bee**: a *participant*. A bee is a key: its `hid` is
  `bee_<sha256(pubkey)>`, filed at `bees/<hid>.key`. Whatever signs a
  hello with that key is the bee, be it a process its hive started, a
  runtime hosting many bees, or a service on a phone. The hello names the
  `hive` it belongs to and the `roles` it claims:
  - **worker**: produces compute. Implements the `Worker` trait (or
    its non-Rust equivalent), accepts `chi:"prompt"` for advertised
    models and emits `chi:"unfurl"` / `chi:"wilt"` / `chi:"tendril-reach"`.
  - **forager**: the bee that prompts. It brings the outside world into
    the thrum, an HTTP / gRPC / SMS / stdio surface, and opens blooms.

  A role is a claim, never part of who the bee is; a bee may claim both,
  or a role a future hive invents.
- **entering**: a hive signing a bee in (`hum bee enter --hive <hive>
  --as <label>`), minting its key and recording it under the hive by a
  label the hive knows it by, so the same label is always the same bee.
  humd admits only a hello signed by an entered key, under the hive it
  entered. **Exiting** signs the bee out (humd refuses its next hello);
  **reentering** signs it back in. Entering starts no process; that is
  the hive's to do.
- **nestler**: a bee's lifecycle phase: handshaking, awaiting the breath
  that accepts `chi:"hello"`. Verb-state of the same participant.
- **nestled**: a bee's lifecycle phase: post-handshake, registered, with
  a nestledId. The bee keeps asking (`chi:"prompt"`, `chi:"cancel"`,
  `chi:"tendril-result"`, `chi:"release-permit"`, `chi:"cleanup"`,
  `chi:"curate"`) for the rest of the connection.

The vocabulary stacks as: **hive** = kind, installed on a humd; **bee** =
a participant the humd understands through its hive; **nestler** and
**nestled** = states of that same bee. Nestler arrives, nestled inhabits:
same actor, two phases. A nestled bee shares the nest with the cells that
live there.

## Observation

- **drone** — the sentinel watching every tone. Self-governance + drift detection.

## Measure

- **comb** — the kernel's ledger of what crossed it. Lifetime counters: tone
  counts per chi (`chi:wilt`), and **nectar** — the raw intake, tokens in
  and out, cache reads and writes, per hive (`hive:claude-cli:nectar:in`).
  Only the kernel writes the comb, so the measure is observed rather than
  self-reported. What the kernel *produces* is honey, and that side belongs
  to whoever earns it.


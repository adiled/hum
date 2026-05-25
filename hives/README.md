---
title: "hives"
description: "catalogue of bee implementations — each subdir is a kind of hive that commissions worker / forager bees handshaking with humd via thrum"
---

# hives

> _catalogue of bee implementations — each subdir is one **hive**: a
> kind/contract that commissions one or more **bees** (workers,
> foragers, or both)_

A **nest** is the space inside a humd where bees gather and cells
live. A **hive** is the *kind* — `claude-cli`, `claude-repl`, future
`ollama-local`, future `openai-server-as-forager`. A **bee** is the
running participant a hive commissions: a **worker** produces compute
(`WorkerBee` trait — `spawn(SpawnSpec) → Cell`), a **forager**
translates an outside wire into thrum (`ForagerBee` trait). Hybrid
bees that are both are perfectly fine.

This directory is the catalogue of in-repo hive impls — one sub-crate
per kind. **Each ships as a standalone binary** that registers with
humd over thrum (`chi:"hello"` with `bee: ["worker"]` or
`["forager"]`). humd never links a hive in at build time; bees attach
at runtime, same shape as foragers on the other side of the wire.

## Current catalogue

| crate | binary | what its worker spawns | propensity |
|---|---|---|---|
| [`claude-cli`](claude-cli) | `claude-cli-worker` | `claude -p` with `stream-json` over a pipe | stateful-session |
| [`claude-repl`](claude-repl) | `claude-repl-worker` | claude in interactive REPL over a PTY | ephemeral-per-call |
| [`common`](common) | — | `serve_worker` helper + `Classifier` regex bank | library |

## What a worker-bee impl owns

Just two methods:

```rust
#[async_trait]
pub trait WorkerBee: Send + Sync {
    fn ephemeral(&self) -> bool;
    async fn spawn(&self, spec: SpawnSpec) -> Result<Cell>;
}
```

`ephemeral()` declares whether the pool evicts the cell after each
`result` (REPL-style: yes; pipe-mode: no). `spawn` turns a high-level
`SpawnSpec` into a running `Cell` — a struct exposing `stdin`,
`events`, `kill`, and an `exited` oneshot.

The trait says nothing about *what* the cell is. A WorkerBee impl might:

- fork a local model binary (the existing claude-cli, claude-repl cases),
- open an HTTPS connection to a cloud LLM API and bridge it into the
  `stdin → events` shape,
- load weights into the same process and run inference in-thread,
- return a canned deterministic response for sim tests (`mock.rs` in
  [`nest/`](../nest) does this).

From the wire's point of view all of these are identical: `chi:"prompt"`
in, `chi:"chunk"` + `chi:"finish"` out. The wire never sees the bee
boundary.

## How a new hive gets on the wire

Hives are thrum-attached processes — same architectural status as
forager hives in [`hives/`](../hives). No humd recompile
required, no PR required.

1. **Write your WorkerBee impl.** Implement `nest::WorkerBee` (defined
   in [`nest/`](../nest)) in your own Rust crate. The trait says how
   to spawn a cell from a `SpawnSpec`; the helper in
   [`common/`](common) (`nest_common::serve_worker`) takes care of
   the thrum loop, hello, prompt dispatch, chunk fan-out, and
   reconnect on socket close.
2. **Ship a binary.** Wrap your impl with `serve_worker(worker, advert)`
   in a `main.rs`. Build the binary; run it.
3. **It registers with humd.** On boot the binary sends a
   `chi:"hello"` with `bee: ["worker"]`, `hive: "<your-kind>"`,
   `models: [...]`, `propensity`, a valid **`hid`** (see below), and
   the canonical chi vocabulary it speaks. humd records the manifest,
   indexed by thrum client_id, and routes future `chi:"prompt"` tones
   with a matching `modelId` to you.

   > **The `hid` is mandatory and must be a real, stable Hid.** Format
   > is `fbee_<hex>` (forager) or `wbee_<hex>` (worker), derived from a
   > **persisted** ed25519 key — *not* an invented string. The thrum
   > client_id is per-connection and changes every reconnect; humd
   > dedupes a bee across reconnects **by its hid**. If the hid is
   > missing, or a placeholder that doesn't parse as a canonical Hid,
   > humd cannot dedupe and **every reconnect leaks a fresh manifest**
   > — you will see your tool count multiply (N tools × number of
   > reconnects) until humd restarts. humd now warns on a
   > missing/invalid hid (`bee.hid.invalid` / `bee.hid.missing`); watch
   > for it. Rust hives get this for free via
   > [`hives/common`](common) identity (persists the key at
   > `~/.local/state/hum/bees/{kind}.key`, derives the Hid from the
   > pubkey). Non-Rust foragers must replicate it: mint once, persist,
   > reuse the same key/hid on every boot.

   For a bee you want to keep alive across reboots and restart cleanly,
   ship a `hives/<kind>/install` modeled on
   [`hives/paid-oracle/install`](paid-oracle/install) — it registers a
   service via [`scripts/svc.sh`](../scripts/svc.sh) as `hum-<kind>`,
   after which `hum bees restart <kind>` bounces it gracefully (same
   identity, no `pkill`).
4. **Mesh discovery is free.** humd gossips your manifest on the
   ensemble's `hum/hives/announce` topic. Peer humds learn you
   exist and can overflow-route their own prompts to your humd.

The `WorkerBee` trait + the `Cell` struct stay as the Rust SDK for
authors who want to build hives in Rust. Non-Rust authors can
implement the same wire role with the per-language thrum-client libs
under [`thrum-clients/`](../thrum-clients) — humd doesn't care which
language your process is in.

## Want a hive in this repo as reference?

Listing in this catalogue is editorial — for hive impls the maintainers
consider exemplars. A PR is optional and unrelated to discoverability.
Your hive is reachable from any humd it handshakes with the moment it
runs.

## See also

- [`nest/`](../nest) — the trait crate. Defines `WorkerBee`,
  `ForagerBee`, `Cell`, `Listener`, `Nest`, `SpawnSpec`, the encoding
  helpers.
- [`hives/`](../hives) — forager hives (the kinds that
  translate outside wire ↔ thrum). Will eventually consolidate into
  `hives/` alongside worker hives.
- [WIRE.md](../WIRE.md) — the "nest model" section explains what
  the wire sees about nests + cells (and what it deliberately doesn't).
- [VOCABULARY](../VOCABULARY.md) — canonical entries for **nest**,
  **cell**, **hive**, **bee**, **worker**, **forager**, **nestler**,
  **nestled**.

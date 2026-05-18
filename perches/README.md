---
title: "perches"
description: "catalogue of Perch implementations — each one is a kind of roost the local humd can spawn into its nest"
---

# perches

> _catalogue of `Perch` implementations — each one is a kind of roost the local humd can spawn into its nest_

A **nest** is the space inside a humd where nestlers and roosts live.
The **roost** is the live LLM subprocess that does the actual compute.
The **`Perch`** trait (defined in [`nest/`](../nest)) is what
declares a *kind* of roost: how to spawn it, what it speaks, when it's
ephemeral.

This directory is the catalogue of concrete Perch impls — one
sub-crate per kind of roost. **Each crate ships as a standalone
binary** that registers with humd over thrum (`chi:"hello"` with
`role:"perch"`). humd never links a perch in at build time; perches
attach at runtime, same shape as nestlings on the other side of
the wire.

## Current catalogue

| crate | binary | what it spawns | propensity |
|---|---|---|---|
| [`claude-cli`](claude-cli) | `claude-cli-perch` | `claude -p` with `stream-json` over a pipe | stateful-session |
| [`claude-repl`](claude-repl) | `claude-repl-perch` | claude in interactive REPL over a PTY | ephemeral-per-call |
| [`common`](common) | — | `serve_perch` helper + `Classifier` regex bank | library |

## What a Perch impl owns

Just two methods:

```rust
#[async_trait]
pub trait Perch: Send + Sync {
    fn ephemeral(&self) -> bool;
    async fn spawn(&self, spec: SpawnSpec) -> Result<Roost>;
}
```

`ephemeral()` declares whether the pool evicts the roost after each
`result` (REPL-style: yes; pipe-mode: no). `spawn` turns a high-level
`SpawnSpec` into a running `Roost` — a struct exposing `stdin`,
`events`, `kill`, and an `exited` oneshot.

The trait says nothing about *what* the roost is. A Perch impl might:

- fork a local model binary (the existing claude-cli, claude-repl cases),
- open an HTTPS connection to a cloud LLM API and bridge it into the
  `stdin → events` shape,
- load weights into the same process and run inference in-thread,
- return a canned deterministic response for sim tests (`mock.rs` in
  [`nest/`](../nest) does this).

From the wire's point of view all of these are identical: `chi:"prompt"`
in, `chi:"chunk"` + `chi:"finish"` out. The wire never sees the Perch
boundary.

## How a new perch gets on the wire

Perches are thrum-attached processes — same architectural status as
nestlings. No humd recompile required, no PR required.

1. **Write your Perch impl.** Implement `nest::Perch` (defined in
   [`nest/`](../nest)) in your own Rust crate. The trait says how to
   spawn a roost from a `SpawnSpec`; the helper in
   [`common/`](common) (`nest_common::serve_perch`) takes care of
   the thrum loop, hello, prompt dispatch, chunk fan-out, and
   reconnect on socket close.
2. **Ship a binary.** Wrap your impl with `serve_perch(perch, advert)`
   in a `main.rs`. Build the binary; run it.
3. **It registers with humd.** On boot the binary sends a
   `chi:"hello"` with `role:"perch"`, `models: [...]`, `propensity`,
   and the canonical chi vocabulary it speaks. humd records the
   manifest, indexed by thrum client_id, and routes future
   `chi:"prompt"` tones with a matching `modelId` to you.
4. **Mesh discovery is free.** humd gossips your manifest on the
   ensemble's `hum/nestlings/announce` topic. Peer humds learn you
   exist and can overflow-route their own prompts to your humd.

The `Perch` trait + the `Roost` struct stay as the Rust SDK for
authors who want to build perches in Rust. Non-Rust authors can
implement the same wire role with the per-language thrum-client libs
under [`thrum-clients/`](../thrum-clients) — humd doesn't care which
language your process is in.

## Want a perch in this repo as reference?

Listing in this catalogue is editorial — for Perch impls the
maintainers consider exemplars. A PR is optional and unrelated to
discoverability. Your perch is reachable from any humd it handshakes
with the moment it runs.

## See also

- [`nest/`](../nest) — the trait crate. Defines `Perch`, `Roost`,
  `Listener`, `Nest`, `SpawnSpec`, the encoding helpers.
- [`nestlings/`](../nestlings) — the other side of the wire. Nestler
  conformances (the kinds of askers).
- [WIRE.md](../WIRE.md) — the "nest model" section explains what
  the wire sees about nests + roosts (and what it deliberately doesn't).
- [VOCABULARY](../VOCABULARY.md) — canonical entries for **nest**,
  **roost**, **Perch**, **nestler**, **nestled**, **nestling**.

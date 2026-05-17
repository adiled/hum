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
sub-crate per kind of roost. A humd loads one or more of these at
build time; its nest can then spawn roosts of those kinds when a
`chi:"prompt"` asks for them.

## Current catalogue

| crate | what it spawns | when humd's nest uses it |
|---|---|---|
| [`claude-cli`](claude-cli) | `claude -p` with `stream-json` over a pipe | normal model-CLI usage; the canonical pipe-mode perch |
| [`claude-repl`](claude-repl) | claude-cli in interactive REPL mode over a PTY | non-stream-json fallback; ephemeral PTY-mode perch |
| [`common`](common) | shared building blocks across nest impls | imports drone's `Classifier` trait, provides the regex pattern bank for context-loss detection. Not a Perch itself |

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

## How a new nest kind gets onto the mesh

Same pattern, no PR to this repo required:

1. **Write your Perch impl** in your own Rust crate. Implement
   `nest::Perch` for your roost kind. The crate can live anywhere.
2. **Build it into your humd.** Add it as a dependency, register the
   impl with the daemon's nest pool at startup. (Future: dynamic
   plugin loading.)
3. **Run your humd.** Its existing `PeerCapabilities.nests` advertise
   gossips your new nest name to the ensemble.
4. **Other humds discover.** A peer humd routing a `chi:"prompt"`
   with `modelId: <your-model>` consults the capability gossip,
   finds your humd, routes the prompt your way.

Nothing new on the wire. Nothing new in the protocol. The `Perch`
trait is the only contract; the rest is humd-local glue.

## Want your Perch listed here as reference?

Listing in this catalogue is editorial — for Perch impls the
maintainers consider exemplars. A PR is optional, unrelated to
discoverability. Your Perch is reachable from the mesh the moment
your humd runs and joins the ensemble, regardless of whether this
README knows about it.

## See also

- [`nest/`](../nest) — the trait crate. Defines `Perch`, `Roost`,
  `Listener`, `Nest`, `SpawnSpec`, the encoding helpers.
- [`nestlings/`](../nestlings) — the other side of the wire. Nestler
  conformances (the kinds of askers).
- [WIRE.md](../WIRE.md) — the "nest model" section explains what
  the wire sees about nests + roosts (and what it deliberately doesn't).
- [VOCABULARY](../VOCABULARY.md) — canonical entries for **nest**,
  **roost**, **Perch**, **nestler**, **nestled**, **nestling**.

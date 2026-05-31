---
title: "humd"
description: "The hum daemon. One per machine install. Holds the nest, owns the thrum socket, joins the ensemble. Built as a binary for production and as a library so simulators and tests can spawn humds in-process."
---

# humd

> _the hum daemon as a binary, and the same boot path as a library so the sim can spawn many of them in one process._

A **humd** is the single process every other piece of hum talks to: bees
hello in over the thrum socket, peer humds dial in over the ensemble
transports, the OS hands it a state directory and a runtime directory,
and it owns one **nest** for the lifetime of the process. Everything
mesh-shaped (routing, gossip, drift, capacity advertise) happens here.
Everything LLM-shaped (worker bees, cells, prompts) happens in the
nest this daemon hosts.

The crate ships two entry points off the same code:

- **binary** `humd`, at `src/main.rs`. What systemd runs.
- **library** `humd::run(cfg, shutdown)`, at `src/lib.rs`. What
  [`sim`](../sim) calls when it spins up many humds in one process to
  exercise the ensemble.

The same `run` function backs both. Tests and simulators never
duplicate the wiring; they hand in a [`DaemonConfig`] with the bits
they want to swap (`thrum_override`, `ensemble`, `capacity_override`,
`waneman`, etc.).

## Boot order

`humd::run` walks a fixed sequence:

1. **paths init** via `hum_paths::init()`, so every other crate that
   reaches for XDG paths agrees on what they resolve to (see
   [`hum-paths`](../hum-paths)).
2. **config validate**. `config::validate_or_exit()` rejects an invalid
   `hum.json` against `hum.schema.json` before any state is built. A
   broken config is a fatal start error, never silently coerced.
3. **identity load**. `identity::load_or_mint_key()` reads
   `$XDG_STATE_HOME/hum/humd.key` (32 raw bytes, mode `0o600`) or mints
   and persists a fresh Ed25519 keypair on first boot.
4. **trackers**. `drift::Drift`, `penny::Penny`, the metrics exporter
   (Prometheus on `humd.metricsAddr`).
5. **thehum**. The signed append-only chi log at
   `$XDG_STATE_HOME/hum/thehum/`. Every tone that crosses the daemon
   lands here as one NDJSON line, hash-chained to its predecessor.
6. **ensemble**. Build an `ensemble::Ensemble` from the persisted
   identity. Bring up iroh (production transport, NodeId pinned to
   `humd_key`), optionally bring up TCP if `humd.tcpListen` is set in
   `hum.json`. Walk `peers.json` and dial outbound. Detach an accept
   loop per transport.
7. **thrum**. Bind the unix socket at
   `$XDG_RUNTIME_DIR/hum/thrum.sock`. Worker and forager bees connect
   here and exchange tones.
8. **MCP**. Bind the HTTP listener at `cfg.http_path` so bees can
   expose tools to their compute via MCP if they want.
9. **wait for shutdown**. SIGTERM, SIGINT, or ctrl-c.

## Inspection surface

Today the binary supports only the minimum CLI handshake:

```bash
humd            # run daemon (default)
humd --version  # print "humd <ver> (thrum <wire ver>)"
humd --help     # print the surface above
```

Richer inspection (`humd peers`, `humd drift`, `humd drone`,
`humd sessions`) plugs in once the daemon exposes an RPC control
socket on `cfg.http_path`. The user-facing CLI is
[`hum`](../hum). It talks to a running humd through the same artifacts
humd writes to disk (`RuntimeInfo`, `peers.json`, thehum log) and,
eventually, through that admin RPC.

## Identity

A humd's `Hid` is `sha256(pubkey)`. The signing key persists at
`$XDG_STATE_HOME/hum/humd.key`. The same `HumdKey` is pinned to iroh's
`SecretKey` at transport bind time, so the QUIC-routed NodeId and the
mesh-level `Hid` derive from the same pubkey. Signed hellos verify
against a single identity, not two correlated ones.

`humd::read_key()` is the read-only loader for inspect tools; it
returns `Ok(None)` when no key file exists yet, instead of minting.

## Transports

humd boots iroh by default (always tried). TCP is opt-in by setting
`humd.tcpListen` to a `host:port` in `hum.json`. Each transport's
inbound listener and outbound dial path live as sibling modules under
[`src/peer_transport/`](src/peer_transport):

```
src/peer_transport/
├─ mod.rs    surface doc, sub-module declarations
├─ iroh.rs   bind / dial_all / spawn_listener over QUIC + NAT punching
└─ tcp.rs    same shape, plain NDJSON over TCP, LAN / loopback use
```

Each accepted or dialed connection lands at `Ensemble::install`
(signed hello, sig-verified inbound), so the peer registry stays
transport-agnostic. The hints a remote peer would paste into its
`peers.json` to dial this humd back are written into
`RuntimeInfo.ensemble_addrs` on disk; `hum ensemble` reads them back.

## DaemonConfig and embedding

`DaemonConfig::from_env()` is production defaults: real XDG paths, mint
identity if absent, bind a real thrum socket, MCP HTTP on `cfg.http_path`,
metrics on `humd.metricsAddr`. Embedders override the bits they need:

| field | who sets it |
|-------|-------------|
| `thrum_override` | sim, to bypass socket binding |
| `ensemble` | sim, to install pre-wired `InMemoryEndpoint` peers |
| `bind_mcp` | sim, to skip the HTTP MCP listener |
| `capacity_override` | sim, to force `Some(0)` for overflow tests |
| `waneman` | sim, to share a `WaneTracker` across humds |
| `humd_key` | tests, to spawn ephemeral identities |
| `bootstrap_peers` | tests, to inject specific `peers.json` rows |

The library face is what makes the `sim` crate possible: many humds in
one process, each with its own `Ensemble`, partitioned and healed by
the test driver, all running the same `run` function the production
binary runs.

## See also

- [`hum-paths`](../hum-paths): the XDG layout humd's filesystem state hangs from.
- [`ensemble`](../ensemble): the mesh humd plugs into. Transports, signed hello, peer registry, gossip, kad.
- [`thrumd`](../thrumd): the unix socket server humd hosts for local bees.
- [`thehum`](../thehum): the signed append-only chi log humd writes every tone into.
- [`sim`](../sim): the in-process multi-humd simulator, calls `humd::run` with overrides.
- [`hum`](../hum): the user-facing CLI that inspects and configures humd.

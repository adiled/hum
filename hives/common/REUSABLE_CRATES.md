# Reusable crates for remote hives

> Status: **implemented on branch `crates/reusable-hive-libs`**.
> Scope: which building blocks a *remote hive* — a hive whose source
> lives in a foreign repo and is shallow-cloned by `hum hive install` —
> should be able to depend on as published crates, instead of reaching
> back into `hum`'s internal tree.
>
> The carve is landed as six commits: the four `hum-*` leaf crates exist,
> `Hid` moved to `ids`, and the tool-surface types unify on `mcp`. The
> daemon tree (`nest`, `drone`, `ensemble`, `mcp`) is unchanged and still
> not imported by remote hives.

## The goal

A hive is a kind + a binary. The whole point of the ensemble is that a
bee on one machine reaches a humd on another, and nothing installs on
the remote humd's disk. So a remote hive's *runtime* only needs the
wire: connect, hello, route tones. It does **not** need the daemon's
in-memory nest, drone, or ensemble machinery.

But today every Rust hive that wants the convenience helpers — identity
minting, `serve_worker` / `serve_forager` loops, MCP bridge, tool-def
shapes — pulls in `nest-common` (`hives/common`), which transitively
drags `nest`, `drone`, `ensemble`, `mcp`, `hum-paths`. That is the
daemon's whole dependency tree for what should be ~four small leaf
crates. A foreign hive shouldn't need `iroh`, `rustls`, `command-group`,
`portable-pty`, `sysinfo`, `metrics` just to mint a bee key and dial a
Unix socket.

This doc evaluates *what* should be carved out, *where* it should live,
and *what stays put*. The boundary is now landed — see **Done** below.

## Current coupling (measured)

`nest-common` (`hives/common`, 1606 lines) is the shared hive library:

| module | lines | pulls in | used by |
|---|---|---|---|
| `serve.rs` (`serve_worker`) | 749 | nest, mcp, hum-paths, ensemble(HidPrefix), ids, lru, metrics | claude-cli, claude-repl, ollama-worker |
| `forager.rs` (`serve_forager`) | 219 | ensemble(HidPrefix), hum-paths, thrum-core | humfs |
| `identity.rs` (`load_or_mint_bee_key`) | 158 | ed25519-dalek, ensemble(Hid), hum-paths, rand | grpc, gsm-modem, bp7, paid-oracle, + all above |
| `mcp_bridge.rs` | 308 | axum, mcp, thrum-core, tokio | claude-cli, claude-repl, ollama-worker |
| `suspicion_regex.rs` | 151 | drone, regex | drone's context-loss heuristic (not hive-facing today) |

The heaviest transitive pull is `ensemble` (iroh QUIC, rustls, tokio-rustls,
onchain reqwest) — yet the hives only touch its `Hid`/`HidPrefix` types, a
~40-line pure-crypto block. `nest` (command-group, portable-pty, sysinfo,
metrics, libc) is only needed by `serve_worker`, not by foragers.

## What should be reusable crates (remote hives)

These are the *wire-facing* primitives a remote hive needs. Each is a leaf
or near-leaf: no daemon state, no transport, no LLM subprocess machinery.

### 1. `hum-identity` — bee identity minting (`identity.rs`)

What remote hives actually need on boot: load-or-mint a 32-byte ed25519
seed at `$XDG_STATE_HOME/hum/bees/<kind>.key`, derive the role-tagged
`fbee_<hex>` / `wbee_<hex>` Hid. This is the **mandatory** hello field —
without it humd can't dedupe across reconnects and leaks a fresh manifest
per reconnect (see hives/README's hid warning).

Today it lives in `nest-common::identity` and pulls `ensemble` (for Hid)
+ `hum-paths`. Both are heavier than needed:
- `Hid`/`HidPrefix` is a 40-line pure sha256+hex type that should move to
  the `ids` crate (which already owns `HumId`) or a new `hum-hid` leaf.
- `hum_paths::bee_key` is one function; the seed format + path must stay
  byte-identical with the TS (`openai-server/src/identity.ts`) and Go
  (`twilio-sms/main.go`) implementations, which already hardcode the path.

**Boundary:** a `hum-identity` crate exposing `BeeKey { signing, hid }`,
`load_or_mint_bee_key(kind, prefix)`, `bee_key_path(kind)`. Deps:
`ed25519-dalek`, `rand`, `ids` (for Hid), `hum-paths` (or inline the path —
see below). No ensemble, no nest, no mcp.

### 2. `hum-thrum` — the wire client (`serve_forager` core + a worker half)

The thrum client loop is the single most reusable thing in the repo: dial
the socket, send `chi:"hello"`, read NDJSON tones, dispatch by chi, ship
results, reconnect forever. Today it's fused into `serve_worker` (nest-
specific) and `serve_forager` (tool-dispatcher-specific), so neither is
usable by a hive that isn't exactly a worker or exactly a forager.

**Boundary:** a transport-agnostic `hum-thrum` crate:
- `connect()` + `hello(advert)` + read/write half split + reconnect loop
  (the pattern in both serve.rs and forager.rs, factored out)
- chi dispatch table (`tool-call`, `prompt`, `cancel`, `tool-result`, …)
  built from `thrum-core::Chi`
- `rid()` already lives in `thrum-core`.
Deps: `tokio`, `serde_json`, `thrum-core`, `hum-paths` (socket path).
Then `serve_worker` / `serve_forager` become thin adapters over it.

### 3. `hum-mcp` — worker-side MCP bridge (`mcp_bridge.rs` + `mcp`)

`mcp` is already a pure library (JSON-RPC envelope, ToolDef, capability
table, tone↔request mapping) — that's the right shape. But the actual
*bridge* (`spawn_local_mcp`, `McpBridge` with pending oneshot resolution)
currently sits in `nest-common::mcp_bridge` with an axum dependency and a
closure callback that ships tones. A worker hive that wants to expose tools
to its compute over MCP needs exactly this.

**Boundary:** fold `mcp_bridge.rs` into the `mcp` crate as a `bridge`
module (axum server + pending map), so `serve_worker` and any remote
worker hive share it. Deps: `mcp`, `axum`, `tokio`, `thrum-core`.

### 4. `hum-tooldef` — tool surface shapes (`ToolDef`, `ToolResult`)

humfs and every forager hive declare tools. Today `ToolDef`/`ToolResult`
are defined in `forager.rs` while `mcp::protocol::ToolDef` is a *different*
type with the same shape. Remote hives need one shared type.

**Boundary:** unify on `mcp::protocol::ToolDef` + `ToolResult` (already
pure) and re-export from a `hum-tooldef` crate (or just from `mcp`).
humfs already imports `ToolDef` from `nest_common`; point it at `mcp`.

### 5. `hum-chi` — the chi registry (already exists: `thrum-core`)

Remote hives need the chi enum, `THRUM_VERSION`, `rid`, `sigil`, envelope,
wane. That's `thrum-core` today — already a leaf (deps: `ids`, sha2, hex,
strum). This is the model for the others: **a remote hive should depend on
`thrum-core` and `hum-identity` and `hum-thrum`, not on `nest-common`.**

`thrum-core` is also the source of truth that codegen fans out to
TS/Python/Go clients — so it must stay in-repo and stay the canonical enum.

## What should NOT be reusable crates

Leave these in the daemon tree; they are not wire-facing and a remote hive
has no business compiling them:

| thing | why it stays |
|---|---|
| `serve_worker`'s nest machinery (`Cell`, `Egg`, `WorkerBee`, LRU cell pool, idle reaper) | daemon-owned compute lifecycle; the `nest` crate is the LLM subprocess pool. A remote hive that *is* a worker implements `WorkerBee` and uses `nest` — that's the point of a worker — but this is the heavy path, and only workers need it |
| `serve_forager`'s `ToolDispatcher` trait + dispatch loop | the per-tool runtime is hive-specific state (cwd, fs roots, permission cache); the *wire* part (see `hum-thrum`) is what's shared |
| `drone` + `suspicion_regex` | sentinel/classifier, daemon-side observability; not hive-facing |
| `ensemble` itself (iroh/rustls/tcp/tls/gossip/kad) | the mesh is humd-to-humd. A bee reaches a *remote* humd over the ensemble *transport* hosted by that humd; the hive never opens the mesh itself. Its only need is `Hid`, which moves to `ids`/`hum-hid` |
| `hum-paths` full | a remote hive needs two paths (`thrum_sock_resolved`, `bee_key`). Either keep `hum-paths` as a tiny leaf (it already is: serde only) or fold those two into `hum-identity`/`hum-thrum` so a hive doesn't import the whole path module. The TS/Go impls already inline these paths, so inlining in Rust keeps parity |

## The dependency ladder

```
thrum-core  (chi, rid, sigil, envelope)          ← leaf, canonical
ids         (HumId + Hid/HidPrefix after move)   ← leaf
hum-paths   (socket + bee-key paths)             ← leaf, serde only
hum-identity  = ids + ed25519-dalek + rand + hum-paths
hum-thrum     = thrum-core + tokio + serde_json + hum-paths
hum-tooldef   = mcp::protocol (pure)
hum-mcp       = mcp + axum + tokio (bridge)

remote hive  →  thrum-core, hum-identity, hum-thrum (+ hum-mcp / hum-tooldef as needed)
daemon tree  →  nest, drone, ensemble, mcp  (unchanged, not imported by remote hives)
```

The win: a remote Rust hive goes from importing `nest-common` (→ ensemble's
iroh/rustls, nest's command-group/portable-pty/sysinfo, metrics) to importing
three or four small leaf crates whose combined deps are tokio + serde +
ed25519 + sha2 + hex + axum (for MCP).

## Done (this branch)

Landed as `c3cd7a6..5cfc393`:

1. `ids` — `Hid`/`HidPrefix`/`HidParseError` moved out of `ensemble`;
   `ensemble` re-exports `ids::{Hid, HidPrefix, HidParseError}` for
   back-compat. `ids` gains a `hex` dep.
2. `hum-identity` — `load_or_mint_bee_key` / `BeeKey` / `bee_key_path`
   carved into a leaf crate (`ids` + `hum-paths` + `ed25519-dalek` +
   `rand`). `hives/common::identity` is a thin re-export shim.
3. `hum-thrum` — the wire client (`connect` / `send_json` /
   `read_tones` / `serve_forever`) carved into a leaf crate
   (`ids` + `hum-paths` + `hum-identity` + `thrum-core` + `tokio`).
   `serve_forager` and `serve_worker` now use it; the chi semantics stay
   in `nest-common`.
4. `hum-mcp` — the axum JSON-RPC bridge (`spawn_local_mcp` /
   `McpBridge` / `handle`) carved into a leaf crate (`mcp` +
   `thrum-core` + `axum` + `tokio`). `hives/common::mcp_bridge` is a thin
   re-export shim. The `mcp` crate stays a pure library (no server).
5. Tool surface unified on `mcp::protocol::{ToolDef, ToolResult}` —
   the `forager.rs` duplicates are gone; `nest-common` re-exports from
   `mcp`. (Chose the doc's "…or just from `mcp`" option — no separate
   `hum-tooldef` crate, since `mcp` is already pure.)

Remaining from the original proposal (not carved yet, still in
`nest-common`): `suspicion_regex.rs` (sentinel heuristic — not hive-
facing), and the `serve_worker` nest machinery (`Cell`/`Egg`/LRU pool —
daemon-owned compute lifecycle, only workers need it).

## Migration

1. Move `Hid`/`HidPrefix` from `ensemble/src/lib.rs` into `ids` (or a new
   `hum-hid`); re-export from `ensemble` for back-compat so existing
   `ensemble::Hid` call sites keep compiling.
2. Carve `identity.rs` → `hum-identity` crate.
3. Factor the thrum client loop out of `serve.rs`/`forager.rs` → `hum-thrum`.
4. Fold `mcp_bridge.rs` into `mcp` as `bridge`.
5. Re-point every hive's `nest-common::{...}` import at the new crates;
   keep `nest-common` as a thin re-export shim during the transition so
   the non-Rust hives' docs (which reference `nest_common::load_or_mint_bee_key`)
   keep resolving.
6. Add each new crate to `[workspace] members` + `[workspace.dependencies]`.

Each new crate is publishable independently (crates.io or a git source),
which is exactly what a remote hive's `Orchfile`/`source` needs: point the
hive at the published `hum-thrum` + `hum-identity` instead of the whole hum
repo.

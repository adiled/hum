---
title: "hives"
description: "how to build a hive, the kind a bee is commissioned from, and run its bees on a humd via thrum"
---

# hives

> _A hive is a kind. A bee is a running instance of it. This page is both the catalogue of reference hives and the guide to building your own._

## The vocabulary, once

A **nest** is the space inside a humd where bees gather and cells live. A **hive** is the kind, such as `claude-cli`, `openai-server`, or `paid-oracle`. It does not run on its own; it is a typology plus a binary. A **bee** is the running instance a hive commissions. A **worker** produces compute: it accepts `chi:"prompt"` and emits `chunk` then `finish`. A **forager** translates an outside wire into thrum, whether that wire is HTTP, SMS, a smart contract, or a paid API. A bee can be both. While a bee is mid-handshake it is a **nestler**, and once registered it is **nestled**.

A bee is just a process that handshakes with a humd over thrum. The handshake is the registration. Once it completes, humd knows the bee exists, what `chi` it speaks, and how to route to it. humd never links a hive in at build time; bees attach at runtime, in any language.

## Catalogue

| hive | role | binary | wire it speaks |
|---|---|---|---|
| [`claude-cli`](claude-cli) | worker | `claude-cli-worker` | `claude -p` stream-json over a pipe |
| [`claude-repl`](claude-repl) | worker | `claude-repl-worker` | claude REPL over a PTY |
| [`humfs`](humfs) | forager | `humfs-forager` | local filesystem (`fs` capability) |
| [`openai-server`](openai-server) | forager | `openai-server` | OpenAI `/v1/chat/completions` |
| [`anthropic-server`](anthropic-server) | forager | `anthropic-server` | Anthropic `/v1/messages` |
| [`ollama-server`](ollama-server) | forager | `ollama-server` | Ollama `/api/chat` |
| [`vercel-ai`](vercel-ai) | forager | `vercel-ai` | Vercel AI SDK `LanguageModelV3` |
| [`paid-oracle`](paid-oracle) | forager | `paid-oracle` | x402 over tool-call, USDC per call |
| [`grpc`](grpc) | forager | `grpc-forager` | gRPC bidi stream |
| [`twilio-sms`](twilio-sms) | forager | `twilio-sms` | Twilio Messaging webhook |
| [`gsm-modem`](gsm-modem) | forager | `gsm-modem` | GSM AT-command serial SMS |
| [`bp7`](bp7) | forager | `bp7-forager` | Bundle Protocol v7 (DTN) |
| [`common`](common) | library | n/a | `serve_worker` / `serve_forager` plus identity |

Listing here is editorial, reserved for exemplars the maintainers keep. Your hive works the moment it handshakes a humd, whether or not it lives in this repo.

## Propensity, the three axes of a bee

Every bee sits somewhere on three orthogonal axes, declared in its hello's `propensity`. They decide which `chi` it can keep.

The first axis is **statefulness**. A `stateful` bee holds its own session graph. A `convention-stateful` bee keeps no server state but speaks a protocol that implies continuation. A `stateless` bee treats each call as a fresh sid. A `transport-only` bee is pure bytes in and bytes out.

The second axis is **richness**. A `rich` bee hands humd cwd, permissions, MCP configs, and prior petals. A `medium` bee hands tools, system, and content. A `lean` bee hands only content and system. An `opaque` bee passes JSON straight through.

The third axis is **wire**, the outside contract the bee translates to, for example `openai/chat`, `twilio/sms`, `x402/tool-call`, or `grpc/bidi`.

A forager picks a contract that already exists in the wild and hides the rest of thrum behind it. A lean bee drops the tones it cannot express, while a rich one forwards everything.

## Build a hive

### 1. Speak thrum

Connect to the humd socket at `$XDG_RUNTIME_DIR/hum/thrum.sock`, with the env override `HUM_THRUM_SOCK`, and exchange newline-delimited JSON tones. The reference clients are about ninety lines each. In Rust, `nest_common::serve_worker` and `serve_forager` run the whole loop for you, covering hello, prompt dispatch, chunk fan-out, and reconnect, so you implement one trait method and ship a `main.rs`. In TypeScript, Python, and Go, copy any `src/thrum.ts`, `clients/python`, or `clients/go`. humd does not care what language the process is written in.

### 2. Handshake, the hello contract

On connect, send `chi:"hello"` with these fields.

| field | meaning |
|---|---|
| `bee` | role array: `["worker"]`, `["forager"]`, or both |
| `hive` | your kind name, for example `"openai-server"` |
| `hid` | stable key-derived identity, described below, and mandatory |
| `models` | model ids a worker serves; foragers may omit it |
| `tools` | tool defs a forager exposes, each `{name, description, inputSchema}` |
| `provides` | capability tags such as `["fs"]` or `["session"]` |
| `version`, `protoVersion` | your release, and the `THRUM_VERSION` you target |
| `chis` | the chi values you speak; advisory |
| `source` | URL to your hive's repo; informational, for humans only |

> ### The hid is mandatory and must be a real, stable Hid
>
> The format is `fbee_<hex>` for a forager or `wbee_<hex>` for a worker, where the hex is `sha256(ed25519 public key)` derived from a persisted key. It must never be an invented string. The thrum `client_id` is per-connection and changes on every reconnect, so humd dedupes a bee across reconnects by its hid.
>
> If the hid is missing, or if it does not parse as a canonical Hid, humd cannot dedupe and every reconnect leaks a fresh manifest. Your tool count then multiplies, growing by the number of tools on every reconnect, until humd restarts. humd warns about this with `bee.hid.missing` and `bee.hid.invalid`, and it also prunes a bee's manifest on disconnect, but a valid hid is still required for correct identity.
>
> Persist the key at `$XDG_STATE_HOME/hum/bees/<kind>.key` as a raw 32-byte ed25519 seed and reuse it on every boot. The reference implementations all produce a byte-identical hid. In Rust use `nest_common::load_or_mint_bee_key` from [`common`](common). In TypeScript see [`hives/openai-server/src/identity.ts`](openai-server/src/identity.ts). In Go see `beeHid` in [`hives/twilio-sms/main.go`](twilio-sms/main.go).

Once the hello is accepted you are registered, and nothing else is required for single-machine use.

### 3. Translate, or transport

A forager maps incoming tones into its outside contract and back, as in `vercel-ai/src/transform.ts`. A transport simply forwards bytes, as in `grpc`. A worker turns `chi:"prompt"` into `chunk` and `finish` from whatever produces its tokens.

### 4. Tools

Forward each incoming `chi:"tool-call"` to your tool surface, then reply with `chi:"tool-result"` carrying the same `callId`, and humd resumes the parked model. humd already suppresses built-in tools that overlap a capability you list in `provides`, and an asker can also pass `disallowedTools` on the prompt.

### chi cheatsheet

`clients/ts/chi.ts` owns the full registry. Most bees pick a subset.

| subset | chi values |
|---|---|
| inference only | `prompt`, `chunk`, `finish`, `error`, `cancel` |
| plus tools | `tool-call`, `tool-result` |
| plus permissions | `permission-ask`, `release-permit` |
| plus state sync | `hello`, `breath`, `session-ready`, `pulse`, `cleanup` |
| plus observability | `perf-mark`, `drone`, `echo`, `log` |

A lean bee drops the tones it cannot express, while a rich one forwards everything and leans on consumer-side machinery to make sense of it.

## Run a hive

There are three deployment shapes, and the protocol is identical across all of them.

In **local dev** you run `cargo run -p humd` and then launch your bee binary. Both sides resolve the same socket path, so there is no install and no service.

In the **ensemble**, a bee on one machine reaches a humd on another over the ensemble transport. The remote humd sees the same hello and routes normally. A `peers.json` with one bootstrap entry turns this on. Nothing installs on the remote humd's disk, and the `source` URL stays purely informational.

As a **managed service** you keep a bee alive across reboots. Ship an `Orchfile` at your hive root declaring the SERVICE + RUN + RESTART (see any bundled hive for a template). `hum hive install <target>` resolves the target, builds the binary (Cargo / pnpm / Go / `build` script — detected from the marker file present), copies the Orchfile into `~/.config/hum/orch.d/`, and asks [orchd](https://github.com/adiled/orchd) to bring the bee up as a user systemd unit (Linux) or launchd agent (macOS). orchd is the supervisor; from there the CLI drives it.

```
hum hive --list                   # catalogue: installer, configured, running
hum hive <name|path|url> install   # build a hive and register its bee
hum bee  --list                   # every bee with hid, role, models, tools, source, state
hum bee  <name|id> enter           # start a stopped bee
hum bee  <name|id> exit            # stop it, preserving state
hum bee  <name|id> reenter         # graceful restart with the same identity
```

`hum bee reenter` is the supported replacement for `pkill`, because it restarts through orchd and the bee keeps its persisted identity. `hum hive install` accepts the same dialect a bee advertises in its `source`: a bundled name, a local path, or a `github.com/<org>/<repo>/tree/<branch>/<path>` URL. Our own repo resolves to the local checkout, and a foreign one is shallow-cloned. The target must contain an `Orchfile`.

## Discovery, optional

For mesh discovery, the ensemble gossips your manifest on `hum/hives/announce`, and peer humds find you through `ensemble.hive_discover("<kind>")`. See [`ensemble/README.md`](../ensemble/README.md). For a censorship-resistant alternative to gossip, publish your manifest hash to a `HumdRegistry`, as described in [`contracts/`](../contracts/). Both are opt-in, and a solo humd needs neither.

## Versioning

`THRUM_VERSION` lives in `clients/ts/chi.ts` and is independent of any package version. A patch covers additive optional fields. A minor covers a new chi value or a new field that has a backward-compatible path. A major covers a removed or renamed chi, or changed semantics. Each bee pins the version it targets, and humd traces every mismatch.

## See also

- [`nest/`](../nest) defines `WorkerBee`, `Cell`, `SpawnSpec`, and the encoders. Process lifecycle (supervise, tree-kill, reap) lives in `nest::lifecycle`.
- [WIRE.md](../WIRE.md) explains what the wire sees of nests and cells.
- [VOCABULARY](../VOCABULARY.md) holds the canonical entries for nest, hive, bee, worker, forager, nestler, and nestled.

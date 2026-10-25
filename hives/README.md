---
title: "hives"
description: "how to build a hive, install it on a humd, and the bees that humd then understands"
---

# hives

> _A hive is a kind. A bee is a participant, understood through its hive. This page is both the catalogue of reference hives and the guide to building your own._

## The vocabulary, once

A humd is a coordination node: it admits bees, routes between them and gossips with its peers, and hosts no cells. A **nest** is the cells a hive hosts, which it turns on by using [`hum-nest`](../hum-nest). A **hive** is the kind, such as `claude-cli`, `openai-server`, or `paid-oracle`: a directory with an `Orchfile`. It does not run on its own. A **bee** is a participant: a key its hive entered, and whatever signs a hello with that key: a process on this machine, a runtime hosting many bees, a service on a phone. A bee does one job, its **role**, set when its hive enters it. A **forager** prompts: it brings work in from an outside wire, whether that wire is HTTP, SMS, a smart contract, or a paid API, and carries the bloom back. A **nurse** grows blooms: it accepts `chi:"prompt"` and unfurls the bloom in its cells until it wilts. A **builder** lends its host: it runs the thrum's `bash` for the blooms that build there. An **attendant** serves the fixed, named tools it lists. A hive that wants two jobs enters two bees. While a bee is mid-handshake it is a **nestler**, and once registered it is **nestled**.

A honeybee on the plant pot by your bed is still a bee; you just need to know what kind it is to understand it. So it goes here. A humd understands a bee only through its hive, and **installing** a hive on a humd is what teaches the humd that kind. The hive then **enters** bees, signing them in one key each, and a bee nestles by signing its hello with its key, wherever it lands from. humd refuses a hello that is unsigned, from a key never entered, or naming a hive it was never taught. humd never links a hive in at build time and hum never starts a bee; bees attach at runtime, in any language, run however their hive likes.

## Catalogue

| hive | role | binary | wire it speaks |
|---|---|---|---|
| [`claude-cli`](claude-cli) | nurse | `claude-cli-nurse` | `claude -p` stream-json over a pipe |
| [`claude-repl`](claude-repl) | nurse | `claude-repl-nurse` | claude REPL over a PTY |
| [`openai-server`](openai-server) | forager, nurse, attendant | `openai-server` | OpenAI `/v1/chat/completions`, `/v1/responses`, `/v1/completions`, `/v1/embeddings` |
| [`anthropic-server`](anthropic-server) | forager | `anthropic-server` | Anthropic `/v1/messages` |
| [`ollama-server`](ollama-server) | forager, nurse | `ollama-server`, `ollama-nurse` | Ollama `/api/chat`, both ways |
| [`vercel-ai`](vercel-ai) | forager | `vercel-ai` | Vercel AI SDK `LanguageModelV3` |
| [`bash`](bash) | builder | `bash-builder` | a bash shell per sigil, on its own host |
| [`paid-oracle`](paid-oracle) | attendant | `paid-oracle` | x402 over a tendril, USDC per call |
| [`web2`](web2) | attendant | `web2` | OpenAPI and MCP, as one command-line tendril, `wrepl` |
| [`grpc`](grpc) | forager | `grpc-forager` | gRPC bidi stream |
| [`twilio-sms`](twilio-sms) | forager | `twilio-sms` | Twilio Messaging webhook |
| [`gsm-modem`](gsm-modem) | forager | `gsm-modem` | GSM AT-command serial SMS |
| [`bp7`](bp7) | forager | `bp7-forager` | Bundle Protocol v7 (DTN) |
| [`hum-nest`](../hum-nest) | library | n/a | `serve_nurse`: the nest, and the MCP its cells reach tools through |

Listing here is editorial, reserved for exemplars the maintainers keep. Your hive works the moment it handshakes a humd, whether or not it lives in this repo.

## Propensity, the three axes of a bee

Every bee sits somewhere on three orthogonal axes, declared in its hello's `propensity`. They decide which `chi` it can keep.

The first axis is **statefulness**. A `stateful` bee holds its own session graph. A `convention-stateful` bee keeps no server state but speaks a protocol that implies continuation. A `stateless` bee treats each call as a fresh sid. A `transport-only` bee is pure bytes in and bytes out.

The second axis is **richness**. A `rich` bee hands humd cwd, MCP configs, and the blooms before its prompt as its `graft`. A `medium` bee hands tools, system, and content. A `lean` bee hands only content and system. An `opaque` bee passes JSON straight through.

The third axis is **wire**, the outside contract the bee translates to, for example `openai/chat`, `twilio/sms`, `x402/tendril`, or `grpc/bidi`.

A forager picks a contract that already exists in the wild and hides the rest of thrum behind it. A lean bee drops the tones it cannot express, while a rich one forwards everything.

## Build a hive

### 0. Be a hive

Put an `Orchfile` at the root of your hive's directory. That is what makes it a hive, and the directory's name is the hive's name. hum recognises one unit in it, `hive-install`, and nothing else. `hum hive <ref> install` records the hive on the humd and then runs that unit in the foreground, with `HUM_SRC` set to your directory, `HUM_HIVE` to your hive's name, `HUM_HIVE_STATE` to your hive's state directory, and `hum` on its `PATH`; if the unit fails, the hive is not installed. The installed Orchfile carries the same three as `ARG`s, so a unit can say `${HUM_HIVE_STATE}`.

What the unit does is yours: build, fetch a model, start processes and keep them alive with whatever supervisor you like. The bundled hives use one small `install` script each that builds and runs their process under [orchd](https://github.com/adiled/orchd) in their own namespace; the process enters its own bee when it starts. Every other unit in the Orchfile is only what your own tooling makes of it.

```
ARG HUM_SRC=

SERVICE hive-install
RUN ${HUM_SRC}/install
ONESHOT true

SERVICE my-hive
RUN ${HUM_SRC}/target/release/my-bee
RESTART always
```

A hive whose bees live elsewhere (a phone, another machine) can have an Orchfile with no units at all; installing it still teaches the humd the kind, and its bees are entered on the humd they land on.

A hive keeps its state in `$XDG_STATE_HOME/hum/hives/<hive>/` (`${HUM_HIVE_STATE}`) and its config in `$XDG_CONFIG_HOME/hum/hives/<hive>.json`; anything else it writes under a hum root is moved to that root's `tmp/` while humd runs.

### 1. Speak thrum

Use hum's client; a hive writes only what its bee does. In Rust that is `hum_thrum::Bee::link`, or `hum_nest::serve_nurse`, which runs a nurse on it, so you implement one trait method and ship a `main.rs`. In TypeScript, Python, Go and C it is the client in [`thrum-clients/<lang>`](../thrum-clients), which runs `hum bee link` for you. Either way the client enters the bee, signs its hello, reads humd's verdict, reconnects when humd restarts, and checks every tone against the thrum, the same in every language ([WIRE.md, The client](../WIRE.md#the-client)).

### 2. Handshake, the hello contract

Give the client your role and your hello's own fields. It adds `hive`, `role`, `protoVersion` and the signature itself.

| field | meaning |
|---|---|
| `hive` | the hive you belong to, for example `"openai-server"`; it must be installed on the humd you land on; the client sets it |
| `role` | the one job you do: `forager`, `nurse`, `builder` or `attendant`, the one your hive entered you for; the client sets it |
| `hid`, `pubkey`, `signedAt`, `signature` | who you are, proven: the id your hive entered you as, and a signature by its key (below); the client sets them |
| `models` | model ids a nurse serves |
| `tools` | tool defs an attendant serves, each `{name, description, inputSchema}` |
| `builds` | a forager's blooms build where it is: a prompt of its with no `cwd` builds at its humd's home |
| `version`, `protoVersion` | your release, and the `THRUM_VERSION` the client speaks, which it sets |
| `chis` | the chi values you speak; advisory |
| `source` | URL to your hive's repo; informational, for humans only |

> ### A bee signs its hello with the key its hive entered
>
> A bee is entered with `hum bee enter --hive <hive> --role <role> --as <label>`, which mints an ed25519 key, files it at `$XDG_STATE_HOME/hum/bees/<id>.key` (a raw 32-byte seed), records it as entered under the hive for that role with that label, and prints its id: `bee_<hex>`, the sha256 of its public key. The same hive and label are always the same bee, so a process enters itself each time it starts (`--as main`), and a runtime that holds many bees enters each by a label of its own. Entering a label again for another role files it under a new key, so nothing the old one was trusted with carries over. openai-server enters one forager for each of its keys, `key-<name>`. Without `--as`, every call mints a new bee.
>
> On every hello, load the key filed under your id and send `hid` (your id), `pubkey` (hex), `signedAt` (unix ms) and `signature` (hex ed25519 over `hum-bee-socket-v1:<hid>:<signedAt>`). humd admits the hello only if the signature verifies, `signedAt` is fresh, the id is entered and not exited, and it entered under the `hive` and for the `role` you name. `hum bee <id> exit` signs you out, so humd refuses your next hello, and `reenter` signs you back in.
>
> Whoever can read the key file is the bee, so the key is the trust boundary. Two processes signing with one key are one bee: the newer connection replaces the older. hum's client does all of this: it enters the bee under its hive and label for its role and signs every hello, in every language.

Once the hello is accepted you are nestled.

### 2b. Host cells, if you do

A nurse that runs its own model processes (cells) uses `hum_nest::serve_nurse`, and its nest's limits are your hive's to set, in the `nest` section of `~/.config/hum/hives/<hive>.json`:

```json
{ "nest": { "maxCells": 8, "idleMs": 300000, "bounds": { "rssBytes": 2147483648, "fdCount": 1024 } } }
```

`maxCells` caps live cells (least recently used is evicted first), `idleMs` evicts a cell left idle, and `bounds` caps each cell's memory, file descriptors, CPU and wall clock (enforced on Linux). Leave the section out for the defaults: 8 cells, five idle minutes, no bounds. The nurse claims its `headroom` (`freeSlots`, `totalSlots`) on its hello and on a `chi:"pulse"` each time a cell is spawned, evicted or dies, and humd routes prompts to nurses with room. A nurse that hosts cells some other way claims its headroom the same way. A cell works no host of its own: it runs in its nest's directory and reaches `bash` like any other tool.

### 3. Translate, or transport

A forager maps incoming tones into its outside contract and back, as in `vercel-ai/src/transform.ts`. A transport simply forwards bytes, as in `grpc`. A nurse turns `chi:"prompt"` into `unfurl`s and a `wilt` from whatever produces its tokens.

### 4. Tools

An attendant lists the tools it serves in its hello's `tools`; humd hands them to every bloom on its humd and routes each `chi:"tendril-reach"` for one of them to it. A builder serves the thrum's own `bash`, for the blooms that build on its host ([WIRE.md, Tendrils](../WIRE.md#tendrils)). Answer a reach with `chi:"tendril-result"` carrying the same `callId`, or `chi:"error"` under it to refuse, and humd resumes the parked model.

### chi cheatsheet

`thrum-clients/ts/chi.ts` owns the full registry (the Rust enum at `thrum-core/src/chi.rs` is the source of truth; codegen emits the rest). Most bees pick a subset.

| subset | chi values |
|---|---|
| inference only | `prompt`, `unfurl`, `wilt`, `error`, `cancel` |
| plus tendrils | `tendril-reach`, `tendril-result`, `tendril-info`, `tendril-meta` |
| plus state sync | `hello`, `breath`, `pulse`, `cleanup` |
| plus observability | `perf-mark`, `drone`, `echo`, `log` |

A lean bee drops the tones it cannot express, while a rich one forwards everything and leans on consumer-side machinery to make sense of it.

## Run a hive

There are three deployment shapes, and the protocol is identical across all of them.

In **local dev** you run `cargo run -p humd`, install your hive, and launch your binary; it enters its own bee. Both sides resolve the same socket path, so there is no service.

Whatever the shape, the humd a bee lands on must have its hive installed and the bee entered there. `hum hive <ref> install` records the hive at `~/.config/hum/hives.d/<hive>.orch` and runs its `hive-install` unit. hum starts nothing itself; whatever runs is the hive's doing.

In the **ensemble**, a bee nestled on one humd is gossiped to others. A humd routes to a remote bee only when it has that bee's hive installed too, so it knows the kind it is handing work to; it need not have entered any bee of it. A `peers.json` with one bootstrap entry turns the ensemble on, and the `source` URL stays purely informational.

As a **managed service** your hive keeps its processes alive itself. The bundled hives do it with orchd, which the hum installer puts on the machine for any hive to use; nothing in hum calls it.

```
hum hive --list                     # the catalogue, what is installed, bees entered under each
hum hive <name|path|url> install    # teach this humd the hive; runs its hive-install
hum hive <name|path|url> uninstall  # forget it (what the hive started is its own to stop)
hum bee  --list                     # bees entered here, and which have nestled
hum bee  enter --hive <hive> --role <role> --as <label>  # sign a bee in (the same one for one label); prints its id
hum bee  <id> exit                  # sign it out; humd refuses its next hello
hum bee  <id> reenter               # sign it back in
```

`hum hive install` accepts the same dialect a bee advertises in its `source`: a bundled name, a local path, or a `github.com/<org>/<repo>/tree/<branch>/<path>` URL. Our own repo resolves to the local checkout, and a foreign one is shallow-cloned. The target must contain an `Orchfile`.

## Discovery, optional

For mesh discovery, the ensemble gossips your manifest on `hum/hives/announce`, and peer humds find bees of your hive through `ensemble.bee_discover("<hive>")`. See [`ensemble/README.md`](../ensemble/README.md). For a censorship-resistant alternative to gossip, publish your manifest hash to a `HumdRegistry`, as described in [`contracts/`](../contracts/). Both are opt-in, and a solo humd needs neither.

## Versioning

`THRUM_VERSION` is defined in `thrum-core/src/lib.rs` (the Rust source of truth) and propagates verbatim through `thrum-clients/{ts,python,go}/chi.*` via codegen on every cargo build. It is independent of any package version. A patch covers additive optional fields. A minor covers a new chi value or a new field that has a backward-compatible path. A major covers a removed or renamed chi, or changed semantics. Each bee pins the version it targets, and humd traces every mismatch.

## See also

- [`hum-nest/`](../hum-nest) defines `Nurse`, `Egg`, `Cell`, the MCP its cells reach tools through, and the encoders. Process lifecycle (supervise, tree-kill, reap) lives in `hum_nest::lifecycle`.
- [WIRE.md](../WIRE.md) explains what the wire sees of nests and cells.
- [VOCABULARY](../VOCABULARY.md) holds the canonical entries for nest, hive, hive installation, bee, its roles, nestler, and nestled.

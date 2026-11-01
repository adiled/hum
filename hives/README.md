---
title: "hives"
description: "how to build a hive, install it on a humd, and the bees that humd then understands"
---

# hives

> _A hive is a kind. A bee is a participant, understood through its hive. This page is both the catalogue of reference hives and the guide to building your own._

## The vocabulary, once

A humd is a coordination node: it admits bees, routes between them and gossips with its peers, and hosts no cells. A **nest** is the cells a hive hosts, which it turns on by using [`hum-nest`](../hum-nest). A **hive** is the kind, such as `claude-cli`, `openai-server`, or `paid-oracle`: a directory with an `Orchfile`. It does not run on its own. A **bee** is a participant: a key its hive entered, and whatever signs a hello with that key: a process on this machine, a runtime hosting many bees, a service on a phone. A bee does one job, its **role**, set when its hive enters it. A **forager** prompts: it brings work in from an outside wire, whether that wire is HTTP, gRPC, SMS or a DTN bundle, and carries the bloom back. A **nurse** grows blooms: it accepts `chi:"prompt"` and unfurls the bloom in its cells until it wilts. A **builder** lends its host: it runs the thrum's `bash` for the blooms that build there. An **attendant** serves the fixed, named tools it lists, and the scents of the models it names. A hive that wants two jobs enters two bees. While a bee is mid-handshake it is a **nestler**, and once admitted it is **nestled**.

A honeybee on the plant pot by your bed is still a bee; you just need to know what kind it is to understand it. So it goes here. A humd understands a bee only through its hive, and **installing** a hive on a humd is what teaches the humd that kind. The hive then **enters** bees, signing them in one key each, and a bee nestles by signing its hello with its key, wherever it lands from. humd refuses a hello that is unsigned, from a key never entered, or naming a hive it was never taught. humd never links a hive in at build time and hum never starts a bee; bees attach at runtime, in any language, run however their hive likes.

## Catalogue

| hive | role | binary | wire it speaks |
|---|---|---|---|
| [`claude-cli`](claude-cli) | nurse | `claude-cli-nurse` | `claude -p` stream-json over a pipe |
| [`claude-repl`](claude-repl) | nurse | `claude-repl-nurse` | the `claude` REPL over a PTY |
| [`dwarfstar`](dwarfstar) | nurse, one bee per model | `dwarfstar-bee` | the DwarfStar engine (`antirez/ds4`) linked into the bee, on Metal |
| [`ollama-server`](ollama-server) | forager, nurse | `ollama-server`, `ollama-nurse` | Ollama `/api/chat` and `/api/generate`, both ways |
| [`openai-server`](openai-server) | forager, nurse, attendant | `node dist/index.js` | OpenAI `/v1/chat/completions`, `/v1/responses`, `/v1/completions`, `/v1/embeddings` in front, an OpenAI-compatible API behind |
| [`anthropic-server`](anthropic-server) | forager | `node dist/index.js` | Anthropic `/v1/messages`, streamed or not |
| [`vercel-ai`](vercel-ai) | forager | `createHum`, in the app's own process | Vercel AI SDK `LanguageModelV3` |
| [`grpc`](grpc) | forager | `grpc-forager` | gRPC bidi stream, tones carried whole |
| [`twilio-sms`](twilio-sms) | forager | `twilio-sms` | Twilio Messaging webhook |
| [`gsm-modem`](gsm-modem) | forager | `gsm-modem` | GSM AT-command serial SMS |
| [`bp7`](bp7) | forager | `bp7-forager` | Bundle Protocol v7 (DTN) over UDP |
| [`wifi-p2p`](wifi-p2p) | forager | `WifiP2pBeeService`, an Android app | the thrum's stream over WiFi Direct TCP |
| [`bash`](bash) | builder | `bash-builder` | a bash shell per sigil, on its own host |
| [`paid-oracle`](paid-oracle) | attendant | `paid-oracle` | x402 over a tendril, USDC per call |
| [`web2`](web2) | attendant | `web2 serve` | OpenAPI and MCP, as one command-line tendril, `wrepl` |

Listing here is editorial, reserved for exemplars the maintainers keep. Your hive works once a humd has it installed and its bee nestles there, whether or not it lives in this repo. [`hum-nest`](../hum-nest) is not a hive: it is the library a Rust nurse hosts its cells with (`serve_nurse`), and the MCP its cells reach tools through.

## Propensity, the three axes of a bee

Every bee sits somewhere on three orthogonal axes, declared in its hello's `propensity`. They describe which `chi` it can keep. humd carries them in the bee's manifest and gossips them with it, and routes on none of them.

The first axis is **statefulness**. A `stateful` bee holds its own session graph. A `convention-stateful` bee keeps no server state but speaks a protocol that implies continuation. A `stateless` bee treats each call as a fresh sid. A `transport-only` bee is pure bytes in and bytes out.

The second axis is **richness**. A `rich` bee hands humd cwd, MCP configs, and the blooms before its prompt as its `graft`. A `medium` bee hands tools, system, and content. A `lean` bee hands only content and system. An `opaque` bee passes JSON straight through.

The third axis is **wire**, the outside contract the bee translates to, for example `openai/api`, `twilio/sms-webhook`, `x402/tendril`, or `grpc/bidi`.

The values are conventions, not a closed set: `serve_nurse` declares its statefulness as `stateful_session`, `stateless_per_call` or `ephemeral_per_call`, and its hive's name as its wire.

A forager picks a contract that already exists in the wild and hides the rest of thrum behind it. A lean bee drops the tones it cannot express, while a rich one forwards everything.

## Build a hive

### 0. Be a hive

Put an `Orchfile` at the root of your hive's directory. That is what makes it a hive, and the directory's name is the hive's name, a plain name of letters, digits, `-`, `_` and `.` that does not start with `.`. hum recognises one unit in it, `hive-install`, and nothing else. `hum hive <ref> install` records the hive on the humd and then runs that unit's `RUN` with `sh -c` in the foreground, in the unit's `WORKDIR` or else your directory, with the unit's `ENV`, `HUM_SRC` set to your directory, `HUM_HIVE` to your hive's name, `HUM_HIVE_STATE` to your hive's state directory, and `hum` on its `PATH`; if the unit fails, the hive is not installed. An `ARG` your Orchfile declares takes its value from the environment of `hum hive <ref> install`, which the unit inherits. The copy hum records carries `HUM_SRC`, `HUM_HIVE` and `HUM_HIVE_STATE` as `ARG`s. A supervisor that reads your own Orchfile sees an `ARG` only as you pass it: the bundled `install` passes orchd every `ARG` the Orchfile declares that is set in its environment, which is how a unit there can say `${HUM_HIVE_STATE}`.

What the unit does is yours: build, fetch a model, start processes and keep them alive with whatever supervisor you like. The bundled hives that run a process use one small `install` script each, which runs the hive's `build` and then grows the hive's units under [orchd](https://github.com/adiled/orchd), in the namespace `hum-<hive>` with its state in `${HUM_HIVE_STATE}/orch`: the unit named after the hive, or every unit for a hive with several (dwarfstar, one per model; ollama-server, its forager and its nurse). The process enters its own bee when it starts. A unit's settings are empty `ARG`s passed on as its `ENV`, as in `bp7/Orchfile`, so `BP7_ROUTE=router.local:4556 hum hive bp7 install` sets one, and an empty one keeps the process's own default. vercel-ai, whose bee lives in the app that imports it, only builds. Every other unit in the Orchfile is only what your own tooling makes of it.

```
ARG HUM_SRC=

SERVICE hive-install
RUN ${HUM_SRC}/install
ONESHOT true

SERVICE my-hive
RUN ${HUM_SRC}/target/release/my-bee
RESTART always
```

A hive whose bees live elsewhere (a phone, another machine) can have an Orchfile with no units at all; installing it still teaches the humd the kind. Its bees are entered on the humd they land on, and each key goes to wherever its bee runs, as `wifi-p2p`'s goes to the phone.

A hive keeps its state in `$XDG_STATE_HOME/hum/hives/<hive>/` (`${HUM_HIVE_STATE}`) and its config in `$XDG_CONFIG_HOME/hum/hives/<hive>.json`. Anything else it writes under a hum root is moved to that root's `tmp/` while humd runs (in the config root, once it has sat untouched for ten minutes), and on to `~/.hum-archive/` after a week there.

### 1. Speak thrum

Use hum's client; a hive writes only what its bee does. In Rust that is `hum_thrum::Bee::link`, or `hum_nest::serve_nurse`, which runs a nurse on it, so you implement `Nurse` (`raise`, which turns an `Egg` into a `Cell`, and `ephemeral`) and ship a `main.rs`. A Rust hive is a crate of its own, with its own `[workspace]`, that takes hum's crates as dependencies; humd never builds it. In TypeScript, Python, Go and C it is the client in [`thrum-clients/<lang>`](../thrum-clients), which runs `hum bee link` for you and finds `hum` at `$HUM_CLI`, else on `PATH`, else at `~/.local/bin/hum`. Either way the client enters the bee, signs its hello, reads humd's verdict, reconnects when humd restarts, and checks every tone against the thrum, the same in every language ([THRUM.md, The client](../THRUM.md#the-client)). A refusal is final: the client stops, and the bee then exits non-zero, so that whatever supervises it decides what comes next. A bee that cannot run hum's client, such as `wifi-p2p` on a phone, speaks the wire itself ([THRUM.md, Speaking the wire yourself](../THRUM.md#speaking-the-wire-yourself)).

### 2. Handshake, the hello contract

Give the client your role and your hello's own fields. It adds `chi`, `rid`, `hive`, `role`, `protoVersion` and the signature itself, and ignores any value you give for them with the `client-owned` advisory.

| field | meaning |
|---|---|
| `hive` | the hive you belong to, for example `"openai-server"`; it must be installed on the humd you land on; the client sets it |
| `role` | the one job you do: `forager`, `nurse`, `builder` or `attendant`, the one your hive entered you for; the client sets it |
| `hid`, `from`, `pubkey`, `signedAt`, `signature` | who you are, proven: the id your hive entered you as, and a signature by its key (below); the client sets them |
| `models` | a nurse: the model ids it grows; an attendant: the models it distils a `scent` by |
| `tools` | an attendant: the tool defs it serves, each `{name, description, inputSchema}`; humd ignores them from any other role |
| `builds` | a forager: its blooms build where it is, so a prompt of its with no `cwd` builds at its humd's home |
| `headroom` | a nurse: `{freeSlots, totalSlots}`, how many more cells it can take and how many it holds at most |
| `propensity` | `{statefulness, richness, wire}` (above) |
| `chis` | the chi values you speak; humd sends a bee that lists them only those among the tones it fans out to a sigil, and a bee that lists none every one |
| `version`, `protoVersion` | your release, and the `THRUM_VERSION` the client speaks, which it sets |
| `source` | URL to your hive's repo; informational, for humans only |
| `bind` | an address the bee may be reached at, `{host, port, scheme}`, carried in its manifest |

> ### A bee signs its hello with the key its hive entered
>
> A bee is entered with `hum bee enter --hive <hive> --role <role> --as <label>` on a humd where its hive is installed. The first time, it mints an ed25519 key, files it at `$XDG_STATE_HOME/hum/bees/<id>.key` (a raw 32-byte seed, mode 0600), records it in `bees.d/<id>.json` beside it as entered under the hive for that role with that label, and prints its id: `bee_<hex>`, the sha256 of its public key. The same hive, label and role are always the same bee, so a process enters itself each time it starts (`--as main`), and a runtime that holds many bees enters each by a label of its own. Entering a label again for another role files it under a new key and exits the old one, so nothing the old one was trusted with carries over. openai-server enters one forager for each of its keys, `key-<name>`. Without `--as`, every call mints a new bee.
>
> On every hello, load the key filed under your id and send `hid` (your id), `pubkey` (hex), `signedAt` (unix ms) and `signature` (hex ed25519 over `hum-bee-socket-v1:<hid>:<signedAt>`). humd admits the hello only if its `protoVersion` shares humd's major, its hive is installed, the signature verifies, `signedAt` is within a minute of humd's clock, the id is entered and not exited, it entered under the `hive` and for the `role` you name, and it is not revoked (`humctl revoke`, read when humd starts). `hum bee <id> exit` signs you out, so humd refuses your next hello, and `reenter` signs you back in. A bee nestled when it exits stays until it reconnects. Entering by label signs an exited bee back in too, so a bee whose process enters itself returns the next time that process starts.
>
> Whoever can read the key file is the bee, so the key is the trust boundary. Two processes signing with one key are one bee: the newer connection replaces the older. hum's client does all of this: it enters the bee under its hive and label for its role and signs every hello, in every language.

Once the hello is accepted you are nestled. A change to your hello, new models or new tools, goes out through the client (`Bee::hello`, or `{"hello":{..}}` to the link) as a fresh signed hello on the same connection.

### 2b. Host cells, if you do

A Rust nurse that runs its own model processes (cells) uses `hum_nest::serve_nurse`, and its nest's limits are your hive's to set, in the `nest` section of `$XDG_CONFIG_HOME/hum/hives/<hive>.json`:

```json
{ "nest": { "maxCells": 8, "idleMs": 300000, "bounds": { "rssBytes": 2147483648, "fdCount": 1024 } } }
```

`maxCells` caps live cells (least recently used is evicted first), and `idleMs` evicts a cell left idle. `bounds` (`rssBytes`, `fdCount`, `cpuSecs`, `wallClockMs`, `nice`) rides on each `Egg`. The nest enforces `wallClockMs` itself, on every platform: a cell still alive that long after it was raised is evicted, and a bloom it had not wilted wilts `error`. The rest a nurse applies to its cell's process, with `Bounds::apply_pre_exec` before it execs or `Bounds::apply_to(pid)` just after, and a cell they cannot be applied to is not raised. On Linux, per process and inherited by its children: `rssBytes` is RLIMIT_AS, the address space, so every mapping counts, touched or not, and an allocation past it fails (set it well above what a runtime reserves); `fdCount` is RLIMIT_NOFILE; `cpuSecs` is RLIMIT_CPU, CPU seconds over the process's whole life, past which it is killed; and `nice` is the absolute priority, which a cell can lower only with CAP_SYS_NICE. Elsewhere they do nothing. Of the bundled nurses, claude-cli and claude-repl apply them. Leave the section out for the defaults: 8 cells, five idle minutes, no bounds; a section that does not parse gets the defaults too, with a warning.

The nurse claims its `headroom` (`freeSlots`, `totalSlots`) on its hello and on a `chi:"pulse"` each time a cell is spawned, evicted or dies. humd hands a prompt to a nurse here that grows its model and has room, then to one with room on a peer it trusts with nurses, then to a full one here, and refuses it with `no-nurse` when there is none. A nurse that hosts cells some other way, as dwarfstar does, claims its headroom the same way. A cell works no host of its own: it runs in its nest's directory, `${HUM_HIVE_STATE}/nest`, and reaches `bash` like any other tool.

### 3. Translate, or transport

A forager picks each bloom's `sid`, maps what arrives on its wire into a prompt and the bloom's tones back into its outside contract, as in `vercel-ai/src/transform.ts`. Its breath says what it can reach: the `models` nurses grow, the `scents` attendants distil, and the `tools` they serve. A transport simply forwards tones, as in `grpc`. A nurse turns `chi:"prompt"` into `unfurl`s and a `wilt` from whatever produces its tokens.

### 4. Tools

An attendant lists the tools it serves in its hello's `tools`; humd hands them to every bloom on its humd, and on any peer that trusts remote attendants and has its hive installed, and routes each `chi:"tendril-reach"` for one of them to it. humd drops a tool an attendant lists as `bash` or `scent`, the thrum's own tendrils; an attendant whose hello names `models` serves `scent` for them. A builder serves the thrum's own `bash`, for the blooms that build on its host, a peer's among them. A forager serves the tools it put on its prompt's `tools`, and a reach for one comes back to it ([THRUM.md, The tendril](../THRUM.md#the-tendril)). Answer a reach with `chi:"tendril-result"` carrying the same `callId`, with `isError: true` and `output` saying why when the grasp failed or you refuse it, and humd carries the answer back to the cell that reached. A reach that came from a peer carries that peer's `from`, so a bee can answer its own humd's blooms and not a peer's.

### chi cheatsheet

`hum.thrum` at the repo root holds the full range, and codegen emits it into every client (`thrum-clients/ts/chi.ts` and the rest). What each role sounds and hears, besides the attunement the client keeps for every bee (`hello`, `breath`, `echo`) and `perf-mark` and `log`, which any bee may sound and hear:

| role | sounds | hears |
|---|---|---|
| forager | `prompt`, `cancel`, `cleanup`, `curate`, `scout-result`, `tendril-reach`, `tendril-result`, `attach`, `detach` | `unfurl`, `wilt`, `error`, `pulse`, `scout`, `tendril-reach`, `tendril-meta`, `tendril-info`, `tendril-result` |
| nurse | `unfurl`, `wilt`, `error`, `pulse`, `scout`, `tendril-reach`, `tendril-meta`, `tendril-info` | `prompt`, `cancel`, `cleanup`, `curate`, `scout-result`, `tendril-result` |
| builder | `tendril-result` | `tendril-reach`, `cleanup` |
| attendant | `tendril-result`, `error` | `tendril-reach` |

`drone`, `drone-retrofit` and the ensemble's chis are humd's own. [THRUM.md, Roles](../THRUM.md#roles) has the same table, each chi linked to its body. Most bees keep a subset; a bee that lists `chis` hears only those of a bloom's tones, so it lists every one it wants.

## Run a hive

There are three deployment shapes, and the protocol is identical across all of them.

In **local dev** you run `cargo run -p humd`, install your hive with `hum hive ./path/to/your-hive install`, and launch your process; it enters its own bee. Both sides resolve the same socket ([THRUM.md, The stream](../THRUM.md#the-stream)), so there is no service. A bee in another language needs `hum` where its client looks for it.

Whatever the shape, the humd a bee lands on must have its hive installed and the bee entered there. `hum hive <ref> install` records the hive at `$XDG_CONFIG_HOME/hum/hives.d/<hive>.orch` and runs its `hive-install` unit. A running humd recognises a hive installed after it started at that hive's first hello, and forgets an uninstalled one only when it restarts. hum starts nothing itself; whatever runs is the hive's doing.

In the **ensemble**, a bee nestled on one humd is gossiped to others. A humd routes to a remote bee only when it has that bee's hive installed too, so it knows the kind it is handing work to; it need not have entered any bee of it. It hands a prompt to a peer's nurse only with `humd.trustRemoteNurses` in `hum.json` (or `HUM_TRUST_REMOTE_NURSES=1`), and a reach or a bloom's tools to a peer's attendant only with `humd.trustRemoteAttendants` (or `HUM_TRUST_REMOTE_ATTENDANTS=1`); both are off by default. A humd dials the peers in its `peers.json` (`hum ensemble peer add`), and one entry is enough to join an ensemble. The `source` URL stays purely informational.

As a **managed service** your hive keeps its processes alive itself. The bundled hives do it with orchd, which the hum installer puts at `~/.local/bin/orchd`, pinned, for any hive to use; nothing in hum calls it.

```
hum hive --list                     # the catalogue, what is installed, bees entered under each
hum hive <name|path|url> install    # teach this humd the hive; runs its hive-install
hum hive <name|path|url> uninstall  # forget it; a running humd forgets it at restart, and what the hive started is its own to stop
hum bee  --list                     # bees entered here, and which have nestled
hum bee  enter --hive <hive> --role <role> --as <label>  # sign a bee in (the same one for one label and role); prints its id
hum bee  link --hive <hive> --as <label> --role <role>   # that bee's client on stdio, which a thrum client spawns
hum bee  <id> exit                  # sign it out; humd refuses its next hello
hum bee  <id> reenter               # sign it back in
```

`hum hive install` accepts the same dialect a bee advertises in its `source`: a `https://github.com/<org>/<repo>/tree/<branch>/<path>` URL, a local path, or a bundled name, tried in that order. A bundled name is a directory under the catalogue, the `hives/` of the checkout hum was built in, or else of the installed source clone. Our own repo resolves to that same checkout whatever the branch, and a foreign one is shallow-cloned once to `$XDG_CACHE_HOME/hum/hives/<org>-<repo>-<branch>` and reused from there. The target must contain an `Orchfile`.

## Discovery, optional

For mesh discovery, every humd in an ensemble gossips the manifest of each bee that nestles on it on `hum/hives/announce`, and peer humds find bees of your hive through `ensemble.bee_discover("<hive>")`. See [`ensemble/README.md`](../ensemble/README.md). For a censorship-resistant alternative to gossip, a humd's manifest hash and URI can be committed to any contract implementing `IHumdRegistry`, such as the `HumdRegistry` in [`contracts/`](../contracts/), which `ensemble::onchain` reads; humd itself neither writes nor reads one. A solo humd needs neither.

## Versioning

`THRUM_VERSION` is the `THRUM` line of `hum.thrum`. `thrum-core` is held to it, and codegen carries it verbatim into each client's generated chi file (`thrum-clients/ts/chi.ts`, `python/thrum/chi.py`, `go/thrum/chi.go`, `c/thrum/chi.h`) whenever `thrum-core` builds. It is independent of any package version. A patch covers additive optional fields. A minor covers a new chi value or a new field that has a backward-compatible path. A major covers a removed or renamed chi, a removed field, or changed semantics. A bee speaks the version its client was built with, in its hello's `protoVersion`; humd refuses another major and admits another minor or patch with the `proto-drift` advisory, and rebuilding the hive brings it level.

## See also

- [`hum-nest/`](../hum-nest) defines `Nurse`, `Egg`, `Cell`, `NestConfig`, `Bounds` (`hum_nest::limits`), `serve_nurse` and its `NurseAdvert`, and the MCP its cells reach tools through (`hum_nest::mcp`). Process lifecycle (supervise, tree-kill, reap) lives in `hum_nest::lifecycle`.
- [THRUM.md](../THRUM.md) holds the hello, the range by role, and what the wire sees of nests and cells.
- [GLOSSARY](../GLOSSARY.md) holds the canonical entries for nest, hive, hive installation, bee, its roles, nestler, and nestled.

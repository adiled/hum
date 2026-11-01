---
title: "Thrum Protocol"
description: "the thrum, hum's vibration protocol: what a tone is, the range of chis, the patterns tones make, the laws every node keeps, and the media a tone travels."
---

# Thrum Protocol

> _the thrum, hum's vibration protocol. A tone is the same in every medium: `hum.thrum` says what it is, and this document says what everyone does with it._

A tone is one vibration: a chi from the range, the body that chi
declares, and the envelope every tone carries. Nobody calls anybody. A
bee sounds a tone, and humd carries it to whoever is tuned to hear it;
an answer is an echo of the same `rid`. The tone is the same whether it
travels a Unix socket, the link's pipes, a TCP or QUIC stream between
humds, or a channel inside one. Those are [media](#media), and none of
them is the thrum.

Two documents define it. [`hum.thrum`](./hum.thrum) is what a tone *is*:
the envelope, the range of chis with their bodies, who sounds and hears
each, the shapes and kinds they carry, the patterns and the advisories.
Every table below that reads the file out is generated from it by
`codegen` (between `thrum:` markers in this document's source), and
`cargo run -p codegen -- --check` fails when they differ; on what a tone
is, the file wins. This document is what a party *does* with one, and on
that it wins.

hum speaks the thrum through one client, `hum-thrum`, in Rust: a Rust
bee holds it in-process, and a bee in TS, Python, Go or C reaches it
through `hum bee link` with the client codegen emits for its language
from `hum.thrum` (`thrum-clients/`). A language hum does not generate a
client for can speak [the link](#the-link) or [the wire
itself](#speaking-the-wire-yourself).

## The tone

A tone is a JSON object, flat: the envelope's keys and the body's side by
side. Keys are case-sensitive, camelCase by convention (`protoVersion`,
`sentAt`); the bodies humds sound to each other are snake_case
(`humd_id`, `msg_id`), as `hum.thrum` writes them. The `chi` and every
chi value are kebab-case (`"tendril-reach"`, `"drone-retrofit"`). A type
with a trailing `?` is optional, and an absent optional field is absent
from the wire, not null.

### Envelope

<!-- thrum: envelope -->

| key | type | meaning |
|---|---|---|
| `chi` | `chi` | which vibration this is |
| `rid` | `string` | the echo key: an echo, or a reply, carries the rid of what it answers; never a dedup key |
| `from` | `string?` | who sounded it: a humd's hid in hex when crossing humds, a bee's name on a local socket |
| `to` | `string?` | the humd it is for, in hex; absent when it stays local |
| `sigil` | `string?` | the 12-character hash of the sid, stable across reconnects |
| `sid` | `string?` | the line of blooms it belongs to; the forager picks it |
| `wane` | `uint?` | its place in its sigil's order: a Lamport clock, one per sigil |
| `sentAt` | `int?` | when it was sounded, wall-clock ms, UTC |
| `dusk` | `int?` | when it fades, absolute ms: a tone arriving past it is dropped before anything else is checked |
| `mid` | `string?` | heard at most once: a receiver that has already dispatched this mid drops the repeat |
| `ext` | `{{any}}?` | a bee's own data under its own key, carried untouched |

<!-- /thrum -->

Beyond the envelope, each chi declares its own body: a `prompt` carries
`content`, `modelId`, `cwd` and the rest of [its fields](#prompt). What a
tone may carry beyond that is the [declared keys](#declared-keys) law.

### Sigil

Three deterministic helpers are pinned by the protocol: `sigil`, `rid`,
and the [wane](#wane). `thrum-core` holds them, and anything else that
computes them MUST match byte-for-byte.

```
sigil = lowercase_hex(sha256("sigil:" + sid)[..6])      // 12 chars
```

What ties a run of blooms together, the same on every humd. humd routes a
bloom's tones by it and keys its wane by it; `sid` is the string the
forager chose to draw it from.

### Rid

```
rid = crockford_base32(now_ms[48 bits] ++ random[208 bits])   // 52 chars, a HumId
```

Correlation id, ordered by when it was minted. Format-agnostic on
receive: only the originator's correlation logic cares about the exact
format.

`rid` is correlation, not identity. Three things follow, and all three
are load-bearing:

- A request and its correlated response share one `rid`. `chi:"echo"`
  is the ack *for* an `rid`, so a receiver that deduped on `rid` would
  drop every response as a duplicate of its request.
- `rid` is explicitly format-agnostic and per-originator, so two humds
  can mint the same value. Some bees mint per-session values
  (`p-<sid>`, `prompt-<sid>`) that recur across restarts.
- A sender that means to publish the same content twice is not
  transmitting a duplicate. Repeats are ordinary for the tones gossip
  carries — a heartbeat, a standing overload alert, a retry.

What is heard once is said by `mid`, and when a tone fades by `dusk`:
[Heard once](#heard-once).

## The range

The range is every chi a tone may carry. Each says who sounds it and who
hears it, and direction is sacred: a role sounds and hears what the range
says, for the whole of its life. A chi that crosses humds may travel the
ensemble; the rest stay between a bee and its humd.

<!-- thrum: range -->

| chi | sounded by | heard by | crosses humds | meaning |
|---|---|---|---|---|
| [`hello`](#hello) | any bee | humd |  | announce self: hive, role, signed identity |
| [`prompt`](#prompt) | forager | nurse | yes | open a bloom: its petals, the blooms before it, and how the cell should grow it |
| [`cancel`](#cancel) | forager | nurse | yes | stop the open bloom |
| [`cleanup`](#cleanup) | forager | nurse, builder | yes | drop humd's state for a sigil |
| [`curate`](#curate) | forager | nurse |  | manual compaction request |
| [`scout-result`](#scout-result) | forager | nurse |  | a scout came back |
| [`tendril-result`](#tendril-result) | forager, builder, attendant | nurse, forager | yes | what a tendril grasped |
| [`breath`](#breath) | humd | any bee |  | accept a hello, and keep the bee in step |
| [`unfurl`](#unfurl) | nurse | forager | yes | a petal growing |
| [`wilt`](#wilt) | nurse | forager | yes | close the bloom, with its nectar |
| [`error`](#error) | nurse, attendant, humd | forager | yes | the bloom broke off |
| [`pulse`](#pulse) | nurse | forager, humd | yes | a cell's life, and headroom |
| [`scout`](#scout) | nurse | forager |  | send a scout out |
| [`tendril-reach`](#tendril-reach) | nurse, forager | forager, builder, attendant | yes | a tendril reaching |
| [`tendril-meta`](#tendril-meta) | nurse | forager | yes | what a tendril carries besides its result |
| [`tendril-info`](#tendril-info) | nurse | forager |  | a tendril that already came back, told whole, for showing only |
| [`echo`](#echo) | any bee, humd | any bee, humd |  | delivery ack for a rid |
| [`perf-mark`](#perf-mark) | any bee, humd | any bee, humd | yes | a timing measurement — measured both ways |
| [`log`](#log) | any bee, humd | any bee, humd |  | structured log forwarding |
| [`drone`](#drone) | humd | humd | yes | drone heartbeat |
| [`drone-retrofit`](#drone-retrofit) | humd | humd | yes | drone swallow + retry signal |
| [`peer-add`](#peer-add) | humd | humd |  | register a peer humd, once its hello is verified: a humd's ensemble tells its own kernel |
| [`peer-remove`](#peer-remove) | humd | humd |  | drop a peer humd |
| [`attach`](#attach) | forager | humd | yes | observe a sid hosted on another humd: its reply tones then fan out to the observer too |
| [`detach`](#detach) | forager | humd | yes | stop observing a sid |
| [`wane-sync`](#wane-sync) | humd | humd | yes | reconcile wane after a partition heals: each side sends its snapshot, and each merges by max |
| [`gossip-publish`](#gossip-publish) | humd | humd | yes | ensemble-wide gossip: deduped on msg_id, dispatched by topic, re-fanned to every other peer |
| [`kad-find-node`](#kad-find-node) | humd | humd | yes | Kademlia FIND_NODE: ask a peer for the peers it knows closest to a target |
| [`kad-find-node-resp`](#kad-find-node-resp) | humd | humd | yes | Kademlia FIND_NODE answer: up to K peers closest to the target |
| [`backfill`](#backfill) | humd | humd | yes | replay thehum's chi-log for an author, from a sequence number |
| [`backfill-event`](#backfill-event) | humd | humd | yes | one event of a backfill |

<!-- /thrum -->

### Roles

A bee does one job, its `role`, set when its hive entered it. What a role
sounds and hears is the range, read by role:

<!-- thrum: roles -->

| role | does | sounds | hears |
|---|---|---|---|
| **forager** | opens blooms, bringing the outside world into the thrum | [`hello`](#hello), [`prompt`](#prompt), [`cancel`](#cancel), [`cleanup`](#cleanup), [`curate`](#curate), [`scout-result`](#scout-result), [`tendril-result`](#tendril-result), [`tendril-reach`](#tendril-reach), [`echo`](#echo), [`perf-mark`](#perf-mark), [`log`](#log), [`attach`](#attach), [`detach`](#detach) | [`tendril-result`](#tendril-result), [`breath`](#breath), [`unfurl`](#unfurl), [`wilt`](#wilt), [`error`](#error), [`pulse`](#pulse), [`scout`](#scout), [`tendril-reach`](#tendril-reach), [`tendril-meta`](#tendril-meta), [`tendril-info`](#tendril-info), [`echo`](#echo), [`perf-mark`](#perf-mark), [`log`](#log) |
| **nurse** | grows blooms in its cells; all inference is nursing | [`hello`](#hello), [`unfurl`](#unfurl), [`wilt`](#wilt), [`error`](#error), [`pulse`](#pulse), [`scout`](#scout), [`tendril-reach`](#tendril-reach), [`tendril-meta`](#tendril-meta), [`tendril-info`](#tendril-info), [`echo`](#echo), [`perf-mark`](#perf-mark), [`log`](#log) | [`prompt`](#prompt), [`cancel`](#cancel), [`cleanup`](#cleanup), [`curate`](#curate), [`scout-result`](#scout-result), [`tendril-result`](#tendril-result), [`breath`](#breath), [`echo`](#echo), [`perf-mark`](#perf-mark), [`log`](#log) |
| **builder** | changes the world: runs what a bloom composes on its host, through `bash` | [`hello`](#hello), [`tendril-result`](#tendril-result), [`echo`](#echo), [`perf-mark`](#perf-mark), [`log`](#log) | [`cleanup`](#cleanup), [`breath`](#breath), [`tendril-reach`](#tendril-reach), [`echo`](#echo), [`perf-mark`](#perf-mark), [`log`](#log) |
| **attendant** | serves the fixed, named tools in its hello's `tools`, and the `scent` of the models its hello's `models` name, and nothing else | [`hello`](#hello), [`tendril-result`](#tendril-result), [`error`](#error), [`echo`](#echo), [`perf-mark`](#perf-mark), [`log`](#log) | [`breath`](#breath), [`tendril-reach`](#tendril-reach), [`echo`](#echo), [`perf-mark`](#perf-mark), [`log`](#log) |

<!-- /thrum -->

Every bee sends `chi:"hello"` first and hears `chi:"breath"` accept it.
It is the same bee throughout: a **nestler** before its hello is
accepted, a **nestled** after, and the role doesn't flip when the state
changes. **Nobody is two roles on the same connection.** A hive that
wants a bee for each job enters one for each.

The wire is **opaque to the cell's implementation.** A cell might
be a local subprocess, a Rust struct that wraps an
HTTP client to OpenAI's API, a deterministic mock for sim tests —
the wire sees the same petals unfurl back either way. The kind of
cell (`Nurse` impl, in Rust terms) is the hive's concern,
exposed to the wire only via `hive: "<kind>"` on hello.

### Chis

Each chi, as `hum.thrum` declares it. `sid` is not among a body's
fields: it is an [envelope](#envelope) field, and a tone carries it
alongside any of these.

<!-- thrum: chis -->

#### `hello`

announce self: hive, role, signed identity

`EMITS bee` · `HEARS humd`

| field | type | meaning |
|---|---|---|
| `protoVersion` | `string` | the THRUM version the bee speaks; another major is refused, another minor advised |
| `hive` | `string` | the hive the bee belongs to; it must be installed on the humd |
| `role` | `role` | the one job it was entered for |
| `builds` | `bool?` | a forager whose blooms build where it is |
| `version` | `string?` | the bee's own version, semver |
| `hid` | `string` | who the bee is: the hash of its key |
| `pubkey` | `string` | the bee's ed25519 public key, hex |
| `signedAt` | `int` | when it signed, ms UTC; a stale signature is refused |
| `signature` | `string` | over hid and signedAt, from the key that hashes to hid |
| `models` | `[string]?` | a nurse: the models it grows; an attendant: the models it distils a scent of |
| `tools` | `[any]?` | an attendant: the fixed, named tools it serves |
| `chis` | `[string]?` | the chi values the bee speaks |
| `propensity` | `any?` | the bee's typology: statefulness, richness, wire |
| `headroom` | `headroom?` | a nurse: how many more cells it can take |
| `source` | `string?` | where the bee's source lives, a URL |
| `bind` | `any?` | an address the bee may be reached at, for the ensemble's manifest of it |

#### `prompt`

open a bloom: its petals, the blooms before it, and how the cell should grow it

`EMITS forager` · `HEARS nurse` · `CARRIES ensemble`

| field | type | meaning |
|---|---|---|
| `modelId` | `string?` | the model to grow it; any nurse with room, growing with a model of its own, when absent |
| `cwd` | `string?` | where the bloom builds: hum://<humdid>/<path> |
| `systemPrompt` | `string?` | the words the cell is given before the bloom's |
| `text` | `string?` | the words, when the bloom is only words |
| `content` | `[petal]?` | the petals the bloom opens with |
| `tools` | `[any]?` | the tools the bloom may reach; humd fills it before a nurse sees it |
| `allowedTools` | `[string]?` | of those, the names the bloom may use; all when absent |
| `resume` | `string?` | a cell's own session to grow on, where the hive keeps one |
| `temperature` | `float?` | how warm the sampling is |
| `prune` | `prune?` | cut the unlikely tail |
| `season` | `uint?` | how many tokens the bloom may grow |
| `frost` | `[string]?` | where the bloom stops |
| `seed` | `int?` | a seed, for a bloom grown again the same |
| `trellis` | `any?` | the shape the answer grows along: a schema, or "json" |
| `tendrils` | `tendrils?` | how the bloom reaches its tendrils |
| `sepal` | `sepal?` | how much the cell reasons first |
| `scion` | `string?` | the text that comes after the bloom: it grows what joins where it starts to this |
| `graft` | `[bloom]?` | the blooms of its sigil before it, oldest first, for a cell that did not grow them |

#### `cancel`

stop the open bloom

`EMITS forager` · `HEARS nurse` · `CARRIES ensemble`

No body.

#### `cleanup`

drop humd's state for a sigil

`EMITS forager` · `HEARS nurse builder` · `CARRIES ensemble`

No body.

#### `curate`

manual compaction request

`EMITS forager` · `HEARS nurse`

No body.

#### `scout-result`

a scout came back

`EMITS forager` · `HEARS nurse`

| field | type | meaning |
|---|---|---|
| `callId` | `string?` | the scout it answers |
| `output` | `any?` | what it brought back |

#### `tendril-result`

what a tendril grasped

`EMITS forager builder attendant` · `HEARS nurse forager` · `CARRIES ensemble`

| field | type | meaning |
|---|---|---|
| `callId` | `string` | the reach it answers |
| `output` | `string?` | what it grasped, as text |
| `result` | `any?` | what it grasped, as JSON |
| `isError` | `bool?` | the grasp failed, and output or result says why |
| `title` | `string?` | a line to show for the grasp |
| `metadata` | `any?` | what else it carries: exitCode on a bash grasp |

#### `breath`

accept a hello, and keep the bee in step

`EMITS humd` · `HEARS bee`

| field | type | meaning |
|---|---|---|
| `sigils` | `any` | state per sigil, usually {}; [] on the breath humd gives before it has heard anything, which admits nothing |
| `protoVersion` | `string?` | the THRUM version humd speaks |
| `models` | `[model]?` | for a forager: the models nurses grow |
| `scents` | `[model]?` | for a forager: the scents attendants distil |
| `tools` | `[any]?` | for a forager: the tools attendants serve its blooms |

#### `unfurl`

a petal growing

`EMITS nurse` · `HEARS forager` · `CARRIES ensemble`

| field | type | meaning |
|---|---|---|
| `petal` | `uint` | its index in the bloom |
| `kind` | `petal-kind` | which unit of content is growing |
| `text` | `string?` | words: of a text, sepal or thorn petal |
| `sap` | `string?` | a tendril's input, as it grows |
| `callId` | `string?` | of a tendril petal: the reach it opens |
| `name` | `string?` | of a tendril petal: the tool |
| `grain` | `grain?` | of a pollen petal |
| `propolis` | `string?` | sealing a sepal petal, so it can carry into the next bloom |
| `source` | `any?` | of a waggle petal: where the words came from |
| `brix` | `any?` | of a brix petal: how likely each token was |
| `closed` | `bool?` | once the petal is whole |

#### `wilt`

close the bloom, with its nectar

`EMITS nurse` · `HEARS forager` · `CARRIES ensemble`

| field | type | meaning |
|---|---|---|
| `reason` | `string` | stop; max_tokens when its season ran out; refusal; cancel; or the cell's own word |
| `nectar` | `any?` | what the bloom took: tokens in and out, cache reads and writes |
| `exitCode` | `int?` | the cell process's exit code, when it had one |
| `subtype` | `string?` | the cell's own word for how it closed |

#### `error`

the bloom broke off

`EMITS nurse attendant humd` · `HEARS forager` · `CARRIES ensemble`

| field | type | meaning |
|---|---|---|
| `message` | `string` | what broke |
| `code` | `string?` | no-nurse when no nurse here or on a trusted peer grows the model; nurse-gone from humd when the peer growing it left the ensemble mid-bloom, so it may be grown again; cannot-grow-on from a nurse that grows only from new words; empty-prompt for a prompt with neither words nor a graft |
| `subtype` | `string?` | the cell's own word for what broke |
| `nectar` | `any?` | what the bloom took before it broke off |

#### `pulse`

a cell's life, and headroom

`EMITS nurse` · `HEARS forager humd` · `CARRIES ensemble`

| field | type | meaning |
|---|---|---|
| `kind` | `pulse-kind` | the moment in the cell's life |
| `pid` | `int?` | the cell's process |
| `headroom` | `headroom?` | the nurse's headroom after this |
| `cell` | `string?` | on cell-ready: the cell's id |
| `model` | `string?` | on cell-ready: its model |
| `tools` | `[any]?` | on cell-ready: its tools |

#### `scout`

send a scout out

`EMITS nurse` · `HEARS forager`

| field | type | meaning |
|---|---|---|
| `task` | `string?` | what the scout is sent to do |
| `tools` | `[any]?` | what it may reach |

#### `tendril-reach`

a tendril reaching

`EMITS nurse forager` · `HEARS forager builder attendant` · `CARRIES ensemble`

| field | type | meaning |
|---|---|---|
| `callId` | `string` | minted by the reacher; the result carries it back |
| `name` | `string` | the tool |
| `args` | `any?` | its input, as the tool's schema says |
| `cwd` | `string?` | on a bash reach to a builder: where its shell starts; its host's home directory when absent |

#### `tendril-meta`

what a tendril carries besides its result

`EMITS nurse` · `HEARS forager` · `CARRIES ensemble`

| field | type | meaning |
|---|---|---|
| `callId` | `string?` | the reach it is about |
| `name` | `string?` | the tool |
| `metadata` | `any?` | what it carries |

#### `tendril-info`

a tendril that already came back, told whole, for showing only

`EMITS nurse` · `HEARS forager`

| field | type | meaning |
|---|---|---|
| `callId` | `string` | the reach, already answered |
| `name` | `string?` | the tool |
| `args` | `any?` | what it was given |
| `result` | `any?` | what came back |
| `isError` | `bool?` | it failed |

#### `echo`

delivery ack for a rid

`EMITS bee humd` · `HEARS bee humd`

| field | type | meaning |
|---|---|---|
| `ok` | `bool` | the tone under this rid was taken |
| `error` | `string?` | why not, when ok is false |

#### `perf-mark`

a timing measurement — measured both ways

`EMITS bee humd` · `HEARS bee humd` · `CARRIES ensemble`

| field | type | meaning |
|---|---|---|
| `at` | `int?` | when, ms UTC |
| `note` | `string?` | what was measured |

#### `log`

structured log forwarding

`EMITS bee humd` · `HEARS bee humd`

| field | type | meaning |
|---|---|---|
| `level` | `string?` | trace, debug, info, warn or error |
| `message` | `string?` | the line |
| `fields` | `any?` | structured fields; from humd, an advisory: fields.advisory is its code |

#### `drone`

drone heartbeat

`EMITS humd` · `HEARS humd` · `CARRIES ensemble`

| field | type | meaning |
|---|---|---|
| `health` | `string` | the drone's own word for how it is |
| `rhythm_ms` | `uint?` | ms between beats |
| `pending_echoes` | `[any]?` | rids sounded and not yet echoed |
| `load` | `drone-load?` | what it is carrying |

#### `drone-retrofit`

drone swallow + retry signal

`EMITS humd` · `HEARS humd` · `CARRIES ensemble`

| field | type | meaning |
|---|---|---|
| `sigil` | `string?` | the sigil to retry |
| `reason` | `string?` | why |

#### `peer-add`

register a peer humd, once its hello is verified: a humd's ensemble tells its own kernel

`EMITS humd` · `HEARS humd`

| field | type | meaning |
|---|---|---|
| `humd_id` | `string` | the peer, in hex |
| `hints` | `[any]?` | where it may be reached |

#### `peer-remove`

drop a peer humd

`EMITS humd` · `HEARS humd`

| field | type | meaning |
|---|---|---|
| `humd_id` | `string` | the peer, in hex |

#### `attach`

observe a sid hosted on another humd: its reply tones then fan out to the observer too

`EMITS forager` · `HEARS humd` · `CARRIES ensemble`

| field | type | meaning |
|---|---|---|
| `hearOnly` | `bool?` | hear the sid's tones without taking part |

#### `detach`

stop observing a sid

`EMITS forager` · `HEARS humd` · `CARRIES ensemble`

No body.

#### `wane-sync`

reconcile wane after a partition heals: each side sends its snapshot, and each merges by max

`EMITS humd` · `HEARS humd` · `CARRIES ensemble`

| field | type | meaning |
|---|---|---|
| `snapshot` | `{uint}` | sigil to wane |

#### `gossip-publish`

ensemble-wide gossip: deduped on msg_id, dispatched by topic, re-fanned to every other peer

`EMITS humd` · `HEARS humd` · `CARRIES ensemble`

| field | type | meaning |
|---|---|---|
| `topic` | `string` | what the message is about; subscribers listen by it |
| `payload` | `any` | the message |
| `from` | `string?` | the originator |
| `msg_id` | `string` | minted once per publish by the originator, never derived from the rid or the payload |

#### `kad-find-node`

Kademlia FIND_NODE: ask a peer for the peers it knows closest to a target

`EMITS humd` · `HEARS humd` · `CARRIES ensemble`

| field | type | meaning |
|---|---|---|
| `query_id` | `string` | matches the answer to its query |
| `target` | `string` | a hid, in hex |

#### `kad-find-node-resp`

Kademlia FIND_NODE answer: up to K peers closest to the target

`EMITS humd` · `HEARS humd` · `CARRIES ensemble`

| field | type | meaning |
|---|---|---|
| `query_id` | `string` | the query it answers |
| `closest` | `[any]` | HumdAddrs, closest in XOR distance to the target |

#### `backfill`

replay thehum's chi-log for an author, from a sequence number

`EMITS humd` · `HEARS humd` · `CARRIES ensemble`

| field | type | meaning |
|---|---|---|
| `author` | `string` | a hid |
| `from` | `int?` | the first sequence number wanted |

#### `backfill-event`

one event of a backfill

`EMITS humd` · `HEARS humd` · `CARRIES ensemble`

| field | type | meaning |
|---|---|---|
| `event` | `any` | the event, as thehum logged it |

<!-- /thrum -->

### Shapes

Shapes nested in bodies, not chis of their own.

<!-- thrum: shapes -->

#### `headroom`

how many more cells a nurse can take

| field | type | meaning |
|---|---|---|
| `freeSlots` | `uint` | cells it can still take |
| `totalSlots` | `uint` | cells it can hold at most |

#### `petal`

one unit of content: in a prompt's content, and in a graft

| field | type | meaning |
|---|---|---|
| `kind` | `petal-kind` | which unit of content it is |
| `text` | `string?` | words: of a text, sepal or thorn petal |
| `grain` | `grain?` | of a pollen petal |
| `callId` | `string?` | of a tendril or grasp petal: the reach |
| `name` | `string?` | of a tendril petal: the tool |
| `args` | `any?` | of a tendril petal: its input |
| `result` | `any?` | of a grasp petal: what came back |
| `isError` | `bool?` | of a grasp petal: it failed |
| `propolis` | `string?` | sealing a sepal petal |
| `source` | `any?` | of a waggle petal: where the words came from |
| `brix` | `any?` | of a brix petal: how likely each token was |
| `bouquet` | `[any]?` | of a bouquet petal: the candidates |

#### `grain`

a grain of media

| field | type | meaning |
|---|---|---|
| `kind` | `string` | image, audio or file |
| `mediaType` | `string` | its media type, image/png and the like |
| `data` | `string?` | the bytes, inline |
| `url` | `string?` | or where to fetch them |

#### `prune`

cut the unlikely tail

| field | type | meaning |
|---|---|---|
| `p` | `float?` | the nucleus: the likeliest tokens whose likelihood sums to p |
| `k` | `uint?` | the k likeliest tokens |

#### `tendrils`

how a bloom reaches its tendrils

| field | type | meaning |
|---|---|---|
| `reach` | `string?` | auto, none, required, or a tendril's name |
| `parallel` | `bool?` | whether the bloom may reach several at once |

#### `sepal`

how much the cell reasons first

| field | type | meaning |
|---|---|---|
| `effort` | `string?` | how hard, on the cell's own scale |
| `budget` | `uint?` | how many tokens of reasoning |

#### `bloom`

one bloom of a graft: what its prompt carried, and what grew

| field | type | meaning |
|---|---|---|
| `content` | `[petal]?` | what its prompt carried |
| `grew` | `[petal]?` | what grew |

#### `model`

a model, and the hive that grows it

| field | type | meaning |
|---|---|---|
| `id` | `string` | the model, as its hive names it |
| `hive` | `string` | the hive that grows it |

#### `drone-load`

what a drone is carrying

| field | type | meaning |
|---|---|---|
| `active_sigils` | `uint?` | sigils with a bloom open |
| `inflight_tools` | `uint?` | tendrils reaching |
| `tokens_burned` | `uint?` | nectar so far |

<!-- /thrum -->

### Kinds

Closed sets of strings a field is drawn from.

<!-- thrum: kinds -->

#### `role`

the one job a bee does, set when its hive enters it

| value | meaning |
|---|---|
| `forager` | opens blooms, bringing the outside world into the thrum |
| `nurse` | grows blooms in its cells; all inference is nursing |
| `builder` | changes the world: runs what a bloom composes on its host, through `bash` |
| `attendant` | serves the fixed, named tools in its hello's `tools`, and the `scent` of the models its hello's `models` name, and nothing else |

#### `petal-kind`

which unit of content a petal is

| value | meaning |
|---|---|
| `text` | words, in `text` |
| `tendril` | a brokered tool call: `callId`, `name`, `args`; its input grows as `sap` |
| `grasp` | what a tendril came back with: `callId`, `result`, `isError` |
| `pollen` | a grain of media: `grain` with its `kind` (image, audio, file), `mediaType`, and `data` or `url` |
| `sepal` | the cell's reasoning, in `text`, sealed by `propolis` so it can carry into the next bloom |
| `waggle` | where a petal's words came from: `source` |
| `thorn` | the cell declining, or a filter acting: `text` says why |
| `brix` | how likely each token was: `brix` |
| `bouquet` | several candidate petals for one prompt: `bouquet` |

#### `pulse-kind`

a moment in a cell's life

| value | meaning |
|---|---|
| `cell-spawned` | process created |
| `cell-ready` | brooded: accepting prompts, its id, model and tools known |
| `cell-idle` | the bloom wilted, no listeners |
| `cell-died` | process exited |
| `cell-evicted` | killed to make room |

<!-- /thrum -->

## Patterns

Tones make figures together. The range names three, and the prose under
each says how it goes.

<!-- thrum: patterns -->

| pattern | key | opens with | grows by | closes with | meaning |
|---|---|---|---|---|---|
| **attunement** | the connection | `hello` | `echo` | `breath`, `echo` | a bee's hello and humd's verdict: humd echoes the hello's rid, then breathes to admit the bee, or echoes ok false under the same rid to refuse it; a refusal is final |
| **bloom** | `sid` | `prompt` | `unfurl`, `cancel` | `wilt`, `error` | one turn of conversation: a prompt opens it, each unfurl grows one petal, and it closes when it wilts or breaks off; a cancel closes it early, as a wilt whose reason is cancel |
| **tendril** | `callId` | `tendril-reach` | `tendril-meta` | `tendril-result`, `error` | a tool reaching and grasping, within a bloom or straight from a forager to an attendant |

<!-- /thrum -->

### Attunement

The first tone a nestler sends after `connect()` MUST be `chi:"hello"`.
**This tone is the registration**: the humd that receives it now knows
your bee exists, what chi values you speak, and can route tones
to you, provided the hive your hello names is installed on that humd
(`hum hive <ref> install`) and the hello is signed by a key that hive
entered (`hum bee enter --hive <hive> --role <role>`). A humd understands
a bee only through its hive, and knows it only by its key. The ensemble
layer (peer humds, gossip) and the on-chain layer (`HumdRegistry`) are
additive opt-ins on top.

Until a breath admits it, a connection is heard for nothing but its
`hello`: any other tone is answered with an `echo` whose `ok` is false
under its rid, and an `echo` or a `log` is dropped. One key is one bee:
when a second connection signed by the same key is admitted, humd closes
the first, and frees what it held as a closing connection does.

The handshake doesn't distinguish where the bytes came from. A hello
arriving on humd's local Unix socket and a hello bridged through the
ensemble from a peer humd hit the same code path. Local-dev runs,
local-prod-with-systemd setups, and distributed mesh deployments all
exercise the same `chi:"hello"` shape. See
[hives, Run a hive](./hives/README.md#run-a-hive) for the three
deployment shapes (local dev, the ensemble, a managed service) that
share this single protocol.

```json
{
  "chi": "hello",
  "rid": "hello-1",
  "hive": "<the hive you belong to>",
  "role": "nurse",
  "hid": "bee_<sha256 of your ed25519 pubkey>",
  "pubkey": "<hex ed25519 public key>",
  "signedAt": 1700000000000,
  "signature": "<hex ed25519 over hum-bee-socket-v1:<hid>:<signedAt>>",
  "version": "<your bee's semver>",
  "protoVersion": "<THRUM_VERSION you target>",
  "propensity": {
    "statefulness": "stateless | convention-stateful | stateful | transport-only",
    "richness":     "rich | medium | lean | opaque",
    "wire":         "<freeform tag, e.g. 'openai/chat-completions' or 'x402/tendril'>"
  },
  "chis": ["hello", "prompt", "unfurl", "wilt"],
  "source": "https://github.com/...optional pointer to your repo..."
}
```

- `hive`, `role`, `protoVersion`, `hid`, `pubkey`, `signedAt` and
  `signature` are **required**. The signature proves the hello comes from
  the entered key the `hid` names (see [hives/README.md](hives/README.md)).
- `role` is the one job the bee does, set when its hive entered it:
  `forager`, `nurse`, `builder` or `attendant` ([Roles](#roles)).
- `builds: true` on a forager's hello says its blooms build where it is:
  a prompt of its with no `cwd` builds in the home directory of its humd's
  host ([Where a bloom builds](#where-a-bloom-builds)).
- `chis` (note: plural) is the list of chi values this bee speaks
  and expects to receive. **Distinct from `chi`** at the top level —
  `chi` is the discriminator (this tone IS a hello); `chis` is the
  vocabulary (these are the tones this bee knows). One word per
  concept; never reuse `chi` for a list.
- All other fields — optional but **strongly recommended**: humd uses
  them to build a `BeeManifest` and gossip it to the rest of the
  ensemble on the `hum/hives/announce` topic.
- humd acknowledges the hello under its `rid` (`echo`, `ok: true`), then
  gives its verdict. A breath admits the bee and carries what humd keeps
  it in step with; an `echo` with `ok: false` under the hello's `rid`
  refuses it, and a refusal is final, even one that comes after
  admission. A forager's breath says what it can reach, and humd breathes
  again whenever that changes ([What a forager can
  reach](#what-a-forager-can-reach)).
- A hello with **no** `protoVersion` is **refused**: the bee is not
  registered, not announced to the ensemble, and the connection is closed.
- A hello naming **no** hive, or a hive **not installed** on this humd,
  is refused the same way. So is one that is **unsigned**, whose
  signature fails or is stale, whose key was never entered under that
  hive or has exited, or whose `role` is not the one its key entered for.
- A `protoVersion` whose **major** differs from humd's is **refused** the
  same way.
- A `protoVersion` that differs only in minor or patch is admitted, with
  the `proto-drift` advisory.
- An unparseable `protoVersion` is refused.
- A change to the hello is a fresh signed hello on the same connection:
  at once while nestled, and the moment the bee is admitted when the
  change came while the verdict was pending.

#### What a forager can reach

The breath that admits a forager says what it can reach, and humd
breathes again whenever that changes:

- `models`: what nurses here grow, and those on peers when this humd
  trusts remote nurses, each `{ id, hive }`;
- `scents`: the models attendants here and on trusted peers distil a
  [`scent`](#scent) by, each `{ id, hive }`;
- `tools`: the tools attendants here and on trusted peers serve its
  blooms, as a nurse is handed them, and [`bash`](#bash) besides wherever
  a bloom builds.

```json
{ "chi": "breath", "sigils": {},
  "models": [{ "id": "claude-opus-4-7", "hive": "claude-cli" }],
  "scents": [{ "id": "text-embedding-3-small", "hive": "openai-server" }],
  "tools": [{ "name": "wrepl", "description": "...", "inputSchema": { ... } }] }
```

### The bloom

A bloom is one turn of conversation. It opens with a `prompt` on a `sid`
the forager picks, carrying its petals in `content` and how the cell
should grow it: `temperature`, `prune`, `season`, `frost`, `seed`,
`trellis`, `tendrils`, `sepal`, `scion` ([prompt](#prompt)). What the
cell grows comes back as `unfurl` tones, each growing one petal named by
its [kind](#petal-kind) until the petal is `closed`, and the nectar the
bloom took rides on its `wilt`. A `cancel` closes it early, as a wilt
whose reason is `cancel`; an `error` is the bloom breaking off. A prompt
with no `modelId` goes to any nurse with room, which grows it with a
model of its own, and one no nurse grows is refused with
`code: "no-nurse"`.

A bloom's tones go to the connections that prompted or attached under its
sid, and to no one else; a sid no connection holds reaches no one. A
`cleanup` frees what humd keeps for the sid, and so does the close of the
last connection that held it, so a forager that takes a sid up again on a
new connection `attach`es it first, and names its `cwd` again. When the
peer growing a bloom leaves the ensemble mid-bloom, humd breaks the bloom
off with `code: "nurse-gone"`, and its forager may prompt it again.

#### Graft

A prompt's `graft` is the blooms of its sigil before it, oldest first, as
its forager has them: each the `content` its prompt carried and the
petals that `grew`, its tendrils and their grasps among them. It is for a
cell that did not grow them: a stateless nurse, a fresh cell, another
nurse than last time, a history its forager has edited. A nurse whose
cell grew every bloom the graft holds grows on from its own; any other
grows from the graft, as if its cell had grown it.

A prompt with no `content` opens no new turn: its bloom grows on from
the graft's last bloom, from where that stopped. A last bloom whose
grasps came back is answered, one whose words stopped mid-way is
continued, and a graft of one bloom with nothing in its `content` is
plain text to grow on from. With a `scion`, the bloom grows what goes
between them. A nurse that cannot grow on that way refuses with
`chi:"error"`, and one with nothing to grow on from does the same.

### The tendril

A tendril is a tool call a cell makes, brokered across the wire. A nurse
reaches (`chi:"tendril-reach"`), and the bee that serves the tool answers
with `chi:"tendril-result"` carrying the same `callId`. Three kinds of
bee serve one:

- a **builder** serves [`bash`](#bash), one of the thrum's own tendrils;
- an **attendant** serves the tools it lists in its hello's `tools`, and
  [`scent`](#scent), the other, for the models its hello names;
- a **forager** serves the tools it put on its prompt's `tools`.

When humd hands a nurse a prompt, the prompt's `tools` are everything the
bloom may reach, each name once and the first to claim it keeping it:
`bash` when the bloom builds somewhere, every attendant's here, the
forager's own, and every attendant's on a live peer, when this humd
trusts remote attendants and knows the peer bee's hive. A nurse hands
them to its cell as they are. humd routes each reach by its name: `bash`
to the builder where the bloom builds, any other name to an attendant
serving it here, then back to the forager when it offered the name, then
to the peer whose attendant serves it.

A peer's attendant sees the input of every reach sent to it, and its
tools' descriptions reach every model here, so trusting remote
attendants is a choice, off by default: `humd.trustRemoteAttendants` in
hum.json, or `HUM_TRUST_REMOTE_ATTENDANTS`. A reach that came from a peer
arrives at the attendant with that peer's `from`, so an attendant can
answer its own humd's blooms and not a peer's.

```
forager → humd:   chi:"prompt" { tools: [{ name, description, inputSchema }, ...] }
humd → nurse:     chi:"prompt" { tools: [...the forager's, the attendants', bash] }
nurse → humd:     chi:"tendril-reach" { sid, callId, name, args }
humd → server:    chi:"tendril-reach" { sid, callId, name, args, cwd? }
server → humd:    chi:"tendril-result" { sid, callId, output, isError }
humd → nurse:     chi:"tendril-result" { sid, callId, output, isError }
```

A reach no bee can serve is answered by humd itself, with a
`tendril-result` whose `isError` is true and whose `output` says why, so
the cell does not wait for an answer that cannot come. A bee that cannot
or will not grasp a reach answers it the same way. An `error` sent under
a reach's `callId` reaches the cell as that failed grasp too, its
`message` as the `output`, its `code` and `subtype` as the `result`, and
its `ext` whole.

#### `bash`

The one tendril the thrum declares (`thrum_core::bash_tool`), so every
nurse hands its cell the same tool. A reach carries `command` and an
optional `timeout` in ms (120000 by default, 600000 at most). The builder
runs it in the sigil's shell, which starts where the bloom builds and
keeps its directory and environment from one reach to the next, until
the sigil's `cleanup`. The result's `output` is stdout and stderr
together, its last line naming the exit code when it is not 0;
`metadata.exitCode` is the exit code, and `isError` is true when it is
not 0.

#### `scent`

The thrum's other tendril (`thrum_core::SCENT`). A scent is where a text
sits among others: a vector of numbers near the scents of texts that mean
alike, as a bee knows one flower from another by its smell. Nothing
blooms, and nothing grows: the same texts always give the same scents, so
it is not nursing but a tool, served by an attendant for the models its
hello's `models` name.

A reach carries the `model` to distil by, the texts as `input`, and
optionally the `dimensions` the scents should have. humd routes it by its
`model`: to an attendant here that names it, then to one on a trusted
peer, and answers it unserved otherwise. Its `result` is
`{ scents, nectar }`, one scent per text in order. A forager reaches it
with no bloom between; no bloom is handed it, since a cell makes words
and has no use for one, and no attendant may offer a tool of its own
named `scent` or `bash`.

```
forager → humd:     chi:"tendril-reach" { callId, name: "scent", args: { model, input: [...] } }
humd → attendant:   chi:"tendril-reach" { callId, name: "scent", args }
attendant → humd:   chi:"tendril-result" { callId, result: { scents: [[...], ...], nectar } }
humd → forager:     chi:"tendril-result" { callId, result }
```

#### Where a bloom builds

A prompt's `cwd` says where its bloom builds: `hum://<humdid>/<path>`,
the builder on that humd, its shell starting in `<path>`. The host may be
a full hid, a short one, or an alias from the forager's humd's
`peers.json`; the forager's humd resolves it to a full hid before the
prompt goes anywhere.

- `hum://<humdid>` with no path starts in the home directory of that
  host.
- `hum:///<path>`, with no host, is the forager's own humd.
- No `cwd` builds in the home directory of the forager's own humd, but
  only when the forager's hello says `builds: true`. Otherwise the bloom
  builds nowhere, and its `bash` reaches are answered as unserved.
- A `cwd` that is not a `hum://` address is dropped, with the
  `cwd-not-hum` advisory, and the prompt is taken as having none.

## Laws

What every node does with any tone, in any medium.

### Declared keys

A key is **declared** when it is an envelope key or a FIELD of its chi
in `hum.thrum`; the structs in `thrum-core/src/views.rs` are that file's
Rust reading (`thrum_core::spec`), held to it by `cargo run -p codegen
-- --check`. Anything else is undeclared: no bee should send it and no
humd may rely on it. A bee's private data goes under `ext`. humd checks
every tone it hears and sends. In the field it logs each undeclared key
once (`thrum.tone.undeclared`); in the simulator the test fails. A new
key on the wire starts as a FIELD in `hum.thrum`.

Undeclared is not unknown. Body fields a node does not know MUST be
preserved on forwarding paths (the gRPC bridge, a gossip relay), so a
chi extended after a node was built passes through it whole.

### Heard once

A message that needs at-most-once delivery carries its own
originator-assigned id, minted once per publish and never reused:

| chi | id field | who mints it |
|---|---|---|
| `gossip-publish` | `msg_id` | originator, once per publish |
| any, when at-most-once is wanted | `mid` | originator, once per logical message |

Receivers dedup on that field and never mint one themselves. The
reference `Ensemble::publish` mints `"{origin6}-{ms:x}-{seq:x}"`: a
per-process counter makes each publish distinct regardless of clock or
content, and the origin prefix keeps two humds from colliding.

`mid` extends the same rule to unicast, and the distinction is the
whole design. `mid` answers *which message is this*; `rid` answers
*which conversation is it in*. A response therefore carries the
request's `rid` and its own `mid` — same rid, different mid, both
delivered. That is what a retry looks like too: same rid, same mid,
delivered once.

`mid` is optional, and its absence is meaningful rather than a
shortcoming. A tone with no `mid` makes no at-most-once claim and is
delivered every time it is sent, so a sender may repeat a body as often
as it likes. Suppressing those would be the original mistake in another
guise — dropping a message the sender meant to send. A receiver must
not substitute `rid` for a missing `mid`.

Each receiver enforces `mid` and `dusk` at its own edge, once per hop,
and keeps the two independent:

- `dusk` is checked first, so a dead tone never occupies seen-set
  capacity and cannot displace a live id.
- The `mid` seen-set is separate from the gossip `msg_id` seen-set. A
  `mid` records that this node already *delivered* a tone; a `msg_id`
  also governs whether it is *re-fanned*. Sharing one set would couple
  two unrelated decisions and let one evict the other's entries early.

Content-addressing an id (`sha256(topic:rid:from:payload)`) cannot
work here. It conflates "delivered twice" with "sent twice", and since
a sender typically marks its own publish seen before sending, a repeat
that hashed alike was dropped before it ever left the origin.

`dusk` bounds the window a duplicate can be recognised in at all: a
tone past its `dusk` is dropped on arrival rather than delivered, so
the seen-set only has to remember ids for as long as they can still be
legitimately in flight. A tone with no `dusk` can be re-fanned
arbitrarily late, so such ids are bounded by the seen-set cap instead
of by any deadline.

### Wane

```
wane[sigil] = u64 counter starting at 0
tick(sigil): wane[sigil] += 1; return wane[sigil]
behind(sigil, remote): remote > wane[sigil]
```

One Lamport clock per sigil. Local mutations call `tick`; the new
value rides on the next outgoing tone in `wane`. On receive:

```
if remote_wane > local_wane[sigil]:
    request resync (wane-sync)
    local_wane[sigil] = max(local_wane[sigil], remote_wane)
```

Both ends converge in O(1) round-trips after the link heals.

### Carrying

humd hosts no cells. A **nest** is the cells a hive hosts (live LLM
subprocesses) and the limits it keeps them under, turned on with
hum-nest. The wire never sees a nest as a thing of its own; it sees the
chi traffic that flows through it, and the `headroom` a nurse claims on
its hello and on each `chi:"pulse"` about its cells (`freeSlots`,
`totalSlots`). humd sends a prompt to a nurse with room, local first,
then on a peer, and to a full one only when no other can serve. A cancel
and a cleanup follow a bloom to the peer that grows it.

What the wire *does* see, at the humd level:

- Each humd advertises in its `PeerCapabilities.nests` (gossiped via
  the ensemble) which **kinds** of hive its nest is configured to
  host.
- Each `chi:"prompt"` carries a `modelId`: the forager says which
  model it wants, from the `models` its breath says it can reach
  ([What a forager can reach](#what-a-forager-can-reach)). humd's
  routing picks an appropriate nurse (a local one over thrum, or routes
  to a peer humd whose advertised hives can serve it). A prompt with no
  `modelId` goes to any nurse with room, which grows it with a model of
  its own.

That's the whole "compute discovery" mechanism. Nothing on a bee's
hello claims a nest: the kind is `hive: "<name>"`, and its `role`.
**The advertise lives on the humd, not invented per forager.**

If you want to add a new kind of compute to the mesh, you ship a
`Nurse` impl (any process speaking thrum), point it at a humd, and
the mesh discovers via the existing capability gossip; peers route
prompts your way. The wire stays exactly the same.

**Send deadline.** Every inter-humd send is bounded at 5s
(`ensemble::SEND_TIMEOUT`). A peer that stops reading fills its socket
buffer rather than closing, so the write stalls indefinitely while the
connection still looks healthy to a liveness lease. On timeout humd
closes that connection, counts the event in `send_timeouts`, and lets
the lease mark the peer dead — a bounded send is what keeps one stalled
peer from wedging a fan-out to every other peer. Implementations that
speak this wire should impose an equivalent deadline; a peer that wants
larger tones or a slower link should say so rather than write without
one.

### Versions

`THRUM_VERSION` is the `THRUM` line of `hum.thrum`. A bee says which it
speaks in its hello's `protoVersion`; humd refuses another major and
advises `proto-drift` on another minor or patch
([Attunement](#attunement)). The bumping rules humd applies:

- **patch** — docstring tweaks, additive-optional fields
- **minor** — new chi value, new required field with compat path
- **major** — removed chi, renamed chi, semantics changed

[Version history](#version-history) says what each one changed.

### Advisories

humd refuses what it must (an unsigned hello, a bee its hive did not
enter, a revoked key, an incompatible protocol) and advises on the rest.
An advisory reaches the bee as a `log` tone whose `fields.advisory` is
one of these codes, and hum's client raises some itself before a tone
leaves the bee. humd keeps them per bee in `advisories.json` in the state
root, and `hum doctor` lists them. A refusal is kept there too, as
`refused` with its reason, when the hello names a bee entered on this
humd, so `hum bee --list` says why a bee that keeps dialling is not
nestled; that bee is not heard from as having failed to return.

<!-- thrum: advisories -->

| code | raised by | meaning |
|---|---|---|
| `undeclared-key` | client, humd | a tone carries keys the thrum does not declare for its chi |
| `unknown-chi` | client | a tone's chi is not in the thrum |
| `not-a-tone` | client | a line from the bee is not a JSON object with a chi, and was dropped |
| `client-owned` | client | the bee set a hello field the client owns, and it was ignored |
| `tone-refused` | client | humd refused a tone under a rid the client gave it |
| `dropped-while-away` | client | tones sent while humd was away were dropped |
| `proto-drift` | humd | the bee speaks another minor version of the thrum |
| `reconnect-storm` | humd | the bee said hello 10 times within 10 minutes |
| `did-not-return` | humd | the bee was nestled when humd stopped and had not come back 2 minutes after humd started |
| `cwd-not-hum` | humd | a prompt's `cwd` is not a `hum://` address, and was dropped |
| `refused` | humd | humd refused the bee's hello, and why; the bee heard it in the hello's echo |

<!-- /thrum -->

## Media

A medium carries tones whole and says nothing about them: how a tone is
framed, how large one may be, and how [attunement](#attunement) maps onto
a connection. Three carry the thrum today.

### The stream

One tone per line: NDJSON. One UTF-8-encoded JSON object per line,
separated by `\n` (`0x0A`). No CRLF, no trailing whitespace, no leading
BOM. Lines that don't parse as JSON are dropped silently by humd; clients
should do the same. On a stream between humds a line over 256 KiB
(`ensemble::framing::MAX_FRAME_BYTES`) is dropped whole, and counted.

**Backpressure**: humd reads at line rate; if a writer can't keep up,
the receiver drops oldest unsent frames. Clients SHOULD assume
at-most-once delivery of any single tone.

The stream runs on four carriers:

- **The Unix socket**, between a bee and its humd, at the first of
  - `$HUM_THRUM_SOCK` (explicit override), then `$HUM_SOCKET`
  - the `socket` named in `$XDG_STATE_HOME/hum/runtime.json`
  - `$XDG_STATE_HOME/hum/thrum.sock`

  Nothing authenticates on the socket itself: filesystem permissions on
  the socket file are the trust boundary, and the hello's signature says
  who the bee is.
- **TCP**, between humds, in the clear, on `humd.tcpListen`: for a LAN
  you trust.
- **TLS**, the same stream wrapped in TLS.
- **iroh**, QUIC with Noise, ALPN `hum/0.5`, hole-punching through the
  public relay mesh when bound relayed; the daemon binds direct-only.

Between humds the stream opens with an Ed25519 handshake: each humd
sounds a `hello` signed by its key, carrying `humd_id`, `proto_version`,
`hosts` and `can_relay`. That hello has the peer's own shape, not the
bee's, and is not yet in the range. Everything after it is tones from the
range, and `hum.thrum` marks which may cross (`CARRIES ensemble`); a
liveness lease keeps the connection honest, and the [send
deadline](#carrying) bounds every write.

### The link

`hum bee link` is hum's client as a process: a bee in any language runs
it and speaks to it over two pipes. One JSON object per line, each way. A
line with a `chi` is a tone and passes through unchanged; a line without
one is the link's own.

| direction | line | meaning |
|---|---|---|
| link to bee | `{"link":"ready","linkVersion":2,"thrum":..,"bee":..,"hive":..,"as":..,"role":..}` | first: the bee is entered; a client of another `linkVersion` stops here |
| bee to link | `{"hello":{..}}` | first: the bee's own hello fields; the link connects once it has them |
| link to bee | `{"link":"nestled"}` | humd admitted the bee, at start and after every reconnect |
| link to bee | `{"link":"astray"}` | humd went away; the link is dialing again |
| link to bee | `{"link":"advisory","code":..,"message":..,"from":..,"fields":{..}}` | something to fix ([Advisories](#advisories)) |
| bee to link | `{"hello":{..}}` | later: a change to the hello; a null drops a field |
| link to bee | `{"link":"refused","error":..}` | last: humd refused the bee, or its hive is not installed |

The link owns the hello's `chi`, `rid`, `hive`, `role`, `protoVersion`,
`hid`, `from`, `pubkey`, `signedAt` and `signature`, gives a tone without a
`rid` one, and swallows humd's acknowledgements of the rids it minted.
It exits 0 when the bee closes its stdin, 3 when the bee is refused, 2
on a first line that is not a hello, and 1 otherwise. It ignores
interrupts and terminations: its life is the pipe's.

### Messages

A tone can also travel whole, as one message, with no line to split.
hum's gRPC hive carries it as `Tone { chi, sid, rid, body }`: the JSON of
the tone as `body` bytes, with `chi`, `sid` and `rid` lifted out so a
client can match what comes back without parsing a body; `body` is
authoritative, and the bridge reads it. Inside one humd, the ensemble
hands tones to the kernel as values, and `peer-add` and `peer-remove`
travel only that way. Either way the tone is the same object, and a
medium that carries it whole has no framing to specify.

## The client

A bee does not implement this wire; it uses hum's client. In Rust that
is `hum_thrum::Bee::link`. In TS, Python, Go and C it is the client in
`thrum-clients/<lang>`, generated from `hum.thrum`, which runs
`hum bee link --hive <hive> --as <label> --role <role>` and talks to it
over [the link](#the-link). Either way the same Rust decides everything
below, so every language behaves the same, and
`cargo test -p hum --test conform` holds each of them to it against a
real humd.

### Speaking the wire yourself

A bee in a language hum does not generate a client for can speak the
link, or the wire itself. The wire, as hum's client speaks it:

1. Resolve the socket as [the stream](#the-stream) says, afresh on every
   dial.
2. Connect. humd breathes once (`sigils: []`) before it has heard
   anything; that breath admits nothing.
3. Send the hello, signed as a key the bee's hive entered, under a fresh
   `rid`. humd acknowledges it (`echo`, `ok: true`), then gives its
   verdict: a breath (`sigils: {}`) admits the bee, an `echo` with
   `ok: false` under the hello's `rid` refuses it. A refusal is final,
   even one that comes after admission. A change to the hello is a
   fresh signed hello on the same connection: at once while nestled,
   and the moment the bee is admitted when the change came while the
   verdict was pending.
4. Split incoming bytes on `\n`, parse each line, drop what does not
   parse. Dispatch by `sid`, then by `chi`.
5. Send tones one per line. Give every tone a `rid`. Check it against
   the views first: humd advises on what it does not declare.
6. Outlive humd. When the socket closes, reconnect with backoff (250 ms,
   doubling to 30 s, with jitter) and send a freshly signed hello,
   carrying the latest `headroom`, including any the bee claimed while
   humd was away. Drop the rest of what the bee sends while humd is
   away. A bee that will not reconnect must exit non-zero, so its
   supervisor starts it again; humd advises `did-not-return` on a bee
   that does neither.

## Version history

| THRUM_VERSION | what changed |
|---|---|
| 0.8.0 | hello names its `hive` and its one `role` (`forager`, `nurse`, `builder`, `attendant`; was `roles`, and before that `bee`), both required, and is signed by a key its hive entered for that role; bee hids are `bee_`; `sigil(sid)` drops its nest; a nurse claims `headroom` on hello and `pulse`; humd hosts no cells; `permission-ask` and `release-permit` are gone, and the drone `load`'s `pending_permissions` with them. The thrum declares `bash`, which a builder serves; a prompt's `cwd` is where its bloom builds, `hum://`, and a forager's hello may say `builds`; humd hands a nurse the bloom's tools in the prompt's `tools`, so `provides`, `foragerTools`, `provided` and `disallowedTools` are gone. The wire speaks hum's building blocks: `chunk` is `unfurl` (a petal growing, by kind), `finish` is `wilt`, `usage` is `nectar`, `tool-*` are `tendril-*`, subagents are `scout`s, `session-ready` folds into the `cell-ready` pulse, breath carries `sigils`, and a forager's breath what it can reach (`models`, `scents`, `tools`), breathed again on change; prompt `content` is petals, and a prompt says how its bloom grows, `scion` among it; a prompt's `graft` carries the blooms before it, so petals no longer carry a `role` and `petal-cell` is gone, and a prompt with no content grows on from its graft; the thrum declares `scent`, which an attendant serves by model; a reach for a forager's own tool comes back to the nurse that reached; a cancel and a cleanup follow a bloom to the peer that grows it; a prompt with no `modelId` goes to any nurse with room, and one no nurse grows is refused with `code: "no-nurse"`; `backfill-event`, which humd always sounded, is in the range |
| 0.7.0 | ensemble plumbing chi (peer-add/remove, attach/detach, wane-sync, gossip-publish, kad-find-node, kad-find-node-resp) |
| 0.6.0 | drone, drone-retrofit, perf-mark first-classed |
| 0.5.0 | release-permit, permission-ask, tendril-reach, tendril-result |
| 0.4.0 | tool-call / tool-result / tool-meta first-classed |
| 0.3.0 | breath / pulse / session-ready handshake refinement |
| 0.2.0 | hello / prompt / chunk / finish baseline |

## See also

- [`hum.thrum`](./hum.thrum): the thrum itself, its envelope, range, shapes, kinds, patterns and advisories. The file wins on what a tone is.
- [GLOSSARY.md](./GLOSSARY.md): what each word means.
- [`thrum-core`](./thrum-core): the Rust reading of it (chi enum, views, helpers), held to the file.
- [`hum-thrum`](./hum-thrum): the client, sans-IO at its core, and the link.
- [`thrum-clients`](./thrum-clients): the generated client for TS, Python, Go and C.
- [`codegen`](./codegen): what reads the file out, into the clients and into this document.
- [`ensemble/README.md`](./ensemble/README.md): inter-humd routing.
- [`hives/README.md`](./hives/README.md): roles, propensity axes, and how to build and run a hive.
- [adiled.github.io/hum](https://adiled.github.io/hum/): the docs site.

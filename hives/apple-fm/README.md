---
title: "apple-fm (Rust + Swift)"
description: "Apple's on-device foundation model as a nurse: local, network-free and key-free, through a small Swift bridge to FoundationModels"
---

# apple-fm (Rust + Swift)

> _Apple's on-device foundation model as a nurse: local, network-free and key-free, through a small Swift bridge to FoundationModels_

The hive has one bee, `apple-fm-nurse`, entered as a nurse under the
label `main`. It claims one model id, `apple-3b-on-device`, and grows
each bloom through the language model Apple ships with macOS 26
(`SystemLanguageModel.default`, about 3B parameters, a 4096-token
context), on this Mac itself. Nothing leaves the
machine, and there is no key.

The nurse is Rust, on [`hum-nest`](../../hum-nest). It reaches Apple
through `apple-fm-gen`, a Swift binary built from
[`bridge/gen.swift`](bridge/gen.swift) and run once per bloom. That one
file is the only code in hum that imports an Apple framework: the Rust
side holds no Apple symbol and builds anywhere, and the hive's `build`
refuses any host but an Apple silicon Mac. The hive is its own cargo
workspace, outside the repo root's.

## Propensity

| bee | statefulness | richness | wire |
|---|---|---|---|
| `apple-fm-nurse` | `stateful_session` | none claimed | `apple-fm` |

It speaks what every hum-nest nurse does: `hello`, `prompt`, `cancel`,
`cleanup`, `curate`, `tendril-result`, `unfurl`, `wilt`, `error`,
`tendril-reach`, `tendril-info` and `pulse`.

## The nurse

| env | default | what |
|---|---|---|
| `APPLE_FM_MODEL` | `apple-3b-on-device` | the model id it claims |
| `APPLE_FM_LABEL` | `main` | the label its bee is entered as; a second nurse beside it needs its own |
| `APPLE_FM_BRIDGE` | `apple-fm-gen` beside the nurse's binary | the bridge it runs |
| `APPLE_FM_PROBE_MS` | `60000` | how often a waiting nurse asks again whether the model is ready |

An empty `APPLE_FM_*` variable is no variable too.

**Before it nestles** the nurse asks the bridge whether Apple's model
can run here (`apple-fm-gen --probe`). When it can, the nurse nestles.
While Apple Intelligence is off (System Settings → Apple Intelligence &
Siri) or its model is still downloading, it waits without nestling,
logs `apple-fm.dormant` with the reason once, and asks again every
`APPLE_FM_PROBE_MS`, nestling as soon as the model is ready. On a Mac
that cannot run the model at all it logs `apple-fm.never` and exits 0,
so its unit stays down. A bridge that cannot be run exits it non-zero.

A bloom grows through one run of the bridge:

- **The conversation.** The system prompt becomes Apple's instructions.
  The blooms before the prompt become Apple's transcript, each bloom's
  words as a prompt and the text it grew as the response, and the
  prompt's own words are what the model answers. A prompt that carries
  a graft is grown from its graft. One that carries none, as from a
  forager that sends only a sigil's new words, is grown from the turns
  its cell has grown so far, so a sigil keeps its conversation either
  way. The model's context is 4096 tokens, so the bridge keeps the
  newest turns that leave room for the reply (its `season`, else 1024
  tokens) and drops the older ones, and the cell forgets them too; the
  nurse logs `apple-fm.trimmed`.
- **How it grows.** `temperature` and `season` become Apple's
  `temperature` and `maximumResponseTokens`. A `temperature` of 0
  samples greedily; `prune.k` samples from the top k, else `prune.p`
  from the top p, each with the prompt's `seed`; a `seed` alone samples
  from them all. `frost` is kept by the nurse: the bloom unfurls up to
  the first stop it meets and wilts there, and the bridge is killed.
  `sepal` is ignored, since the model does not think aloud.
- **The reply.** Its words unfurl as a text petal as the model grows
  them, each unfurl only the words new since the last. The bloom wilts
  `stop`, or `max_tokens` when it grew all of its `season`, with Apple's
  token counts as nectar. A reply with no `season` is held to the
  context left after the conversation, so a long one wilts `max_tokens`
  rather than overflowing. A `cancel` kills the bridge and wilts it
  `cancel`. (Counts, and the room the reply is held to, need macOS 26.4
  or later; before it the bridge drops the older half of the turns
  whenever Apple refuses a conversation as too long.)
- **Refused.** Apple's model grows words from words, so a prompt whose
  petals carry pollen is refused with `apple_fm_words_only`, and one
  with a `trellis` with `apple_fm_no_trellis`. The nurse does not grow
  on: a prompt with no words is refused by its nest with
  `cannot-grow-on`, or `empty-prompt` when it carries no graft either.
- **Errors.** Apple's own become `apple_fm_<reason>`:
  `apple_fm_exceeded_context_window` when the prompt alone overflows
  the model's context,
  `apple_fm_guardrail_violation`, `apple_fm_refusal`,
  `apple_fm_unsupported_language`, `apple_fm_rate_limited`,
  `apple_fm_assets_unavailable`, and `apple_fm_unavailable` when the
  model stopped being ready after the nurse nestled. The nurse adds
  `apple_fm_bridge_spawn` when the bridge cannot be run,
  `apple_fm_bridge_died` when it ends without finishing (with its exit
  status and the end of its stderr), and `apple_fm_cell_stilled` for a
  prompt its cell was stilled before it grew.

The nurse offers the model no tools, so its blooms reach no tendrils.

Its nest's limits are in the `nest` section of
`$XDG_CONFIG_HOME/hum/hives/apple-fm.json`. A cell lives for its sigil,
remembering the turns it grew and growing each prompt the nest hands it
there in turn, until the nest ends it: past `maxCells` (default 8, and
the headroom the nurse claims) as the least recently used, after
`idleMs` (default five minutes) idle, or past `bounds.wallClockMs`.
However a cell ends, the bloom growing in it wilts `cancel` and its
bridge is killed. The other `bounds` cap nothing here.

### The bridge

`apple-fm-gen` reads one JSON line on stdin and writes NDJSON on
stdout, each line as soon as it is known:

```
stdin   {"instructions": "...", "turns": [{"prompt": "...", "response": "..."}],
         "temperature": 0.7, "maximumResponseTokens": 256,
         "sampling": {"mode": "greedy" | "top" | "nucleus", "k": 40, "p": 0.9, "seed": 7},
         "prompt": "..."}
stdout  {"event": "delta", "text": "..."}
        {"event": "finish", "reason": "stop", "dropped": 2, "inputTokens": 15, "outputTokens": 75}
        {"event": "error", "code": "guardrail_violation", "message": "..."}
```

Apple streams each snapshot of the reply whole; the bridge writes only
what is new in it. `dropped` counts the oldest turns it left out to fit
the context. `apple-fm-gen --probe` reads nothing and writes
`{"event": "probe", "available": true}`, or `false` with a `reason`:
`apple_intelligence_not_enabled`, `model_not_ready` or
`device_not_eligible`.

```bash
echo '{"prompt": "Say hello in French."}' | target/release/apple-fm-gen
```

## Run

Needs an Apple silicon Mac on macOS 26 or later, with Apple
Intelligence on, and the Command Line Tools' `swiftc` (no Xcode).

```bash
hum hive apple-fm install
```

`install` builds the nurse (`cargo build --release --locked`) and the
bridge beside it (`swiftc`, into `target/release/apple-fm-gen`), and
keeps the Orchfile's `apple-fm` unit running under orchd, in the
`hum-apple-fm` namespace. A setting rides in the environment of the
install:

```bash
APPLE_FM_MODEL=apple-on-device hum hive apple-fm install
```

The nurse runs from its directory too, once the hive is installed on
the humd it lands on (it enters its own bee when it starts):

```bash
cd hives/apple-fm && ./build && target/release/apple-fm-nurse
```

## Test

```bash
cd hives/apple-fm && cargo test
```

The nurse's tests run it as orchd would, against a humd in a root of its
own, a forager that prompts through it, and a stand-in bridge: a prompt
unfurls and wilts back with its nectar, the sigil's next prompt grows on
from what its cell grew, a cancel kills the bridge, a nurse waits
unnestled until the model is ready and then grows, one on a Mac that
cannot run the model exits 0, and one whose bridge cannot run fails.
Its cells' tests cover the turns a cell keeps, a graft that overrides
them and the turns the bridge dropped, a still, a stop, a season that
ran out, a bridge that lingers after it finished, and each way a bridge
can fail; the unit tests cover the request a prompt becomes, its sampling,
what is refused, what the bridge says, and how `frost` holds back what
may be a stop.

Against Apple's own model, on a Mac that can run it:

```bash
./build && cargo test --test live -- --ignored
```

It holds a conversation through humd, streams a count with each unfurl
new, stops at a stop, wilts `max_tokens` at its season, grows a
conversation longer than the model's context from its newest turns, and
cancels.

## Use

Any forager's client, with the model `apple-3b-on-device`. Through
[`ollama-server`](../ollama-server):

```bash
curl http://localhost:14624/api/chat \
  -H "Content-Type: application/json" \
  -d '{"model": "apple-3b-on-device", "messages": [{"role": "user", "content": "ping"}]}'
```

## What is not yet

- **No tools.** FoundationModels can call tools, but the bridge offers
  the model none, so a bloom's tendrils are not reached.
- **No trellis.** FoundationModels can hold a reply to a schema, but the
  bridge grows free text only.
- **Not in the mac desktop bundle.** The hive installs on its own; the
  desktop app does not yet carry it.

## See also

- [`ollama-server`](../ollama-server), [`dwarfstar`](../dwarfstar): sibling nurses for local models.
- [`hum-nest`](../../hum-nest): the nest the nurse runs on.
- [hives](../README.md): how a hive is installed and how its bees enter.
- [Apple's Foundation Models framework](https://developer.apple.com/documentation/foundationmodels).

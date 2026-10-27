---
title: "dwarfstar"
description: "The DwarfStar engine linked into hum nurses, one bee per model, sharing one GPU"
---

# dwarfstar

> _Nurses with the DwarfStar engine inside them, one per model. A prompt tone goes into the model's token stream, and the answer unfurls back as a bloom._

[DwarfStar](https://github.com/antirez/ds4) (ds4) is a narrow, fast
local inference engine for a few open-weight models: DeepSeek V4 Flash
and V4.1 Flash, GLM 5.2, 5.3 and 5.3 Flash, DeepSeek V4 PRO and Qwen3.8
Flash Next. It is a C library with a CLI, a server and an agent on top.

This hive takes the library, not the server. The bee compiles the engine
from a pinned upstream commit and links it in, and speaks thrum through
the [C client](../../thrum-clients/c), which runs `hum bee link` for it.
Nothing is serialised to HTTP and back: the engine's session, KV cache
and token stream are the bee's own.

## Propensity

| statefulness | richness | wire | hides |
|---|---|---|---|
| stateless | lean | `dwarfstar/core` | tendrils, sepals, pollen, and how a prompt asks its bloom to grow |

Each bee is a nurse and speaks `hello`, `prompt`, `cancel`, `unfurl`,
`wilt`, `error` and `pulse`.

## Install

```sh
brew install pkgconf cjson
DWARFSTAR_DIR=~/ds4 hum hive ./hives/dwarfstar install
```

`install` builds the bee and grows every unit of the Orchfile under
orchd, in the `hum-dwarfstar` namespace. Each unit's bee enters itself
under the hive, as the unit's name, when it starts. A unit whose model is
not on this machine logs so and stays down. It builds on macOS, where
ds4's Metal backend is what it links.

## One unit, one model, one bee

| unit | model file (ARG) | answers to |
|---|---|---|
| `ds4flash` | `DS4FLASH_MODEL` | `deepseek-v4-flash` |
| `ds4flash-q2` | `DS4FLASH_Q2_MODEL` | `deepseek-v4-flash-q2` |
| `ds41flash-q2` | `DS41FLASH_Q2_MODEL` | `deepseek-v4.1-flash`, `deepseek-v4.1-flash-q2` |
| `glm53flash-q2` | `GLM53FLASH_Q2_MODEL` | `glm-5.3-flash`, `glm-5.3-flash-q2` |

Every unit runs the same `dwarfstar-bee`, pinned with `DWARFSTAR_MODEL`
and `DWARFSTAR_MODELS`, so each model is its own participant: its own
key, its own ids in its hello, its own headroom. Stopping one unit takes
its model off the mesh and leaves the rest. A relative model file is
under `DWARFSTAR_DIR`. Each ARG defaults to the name ds4's
`download_model.sh` gives that model under `gguf/`, and `DS4FLASH_MODEL`
to `ds4flash.gguf`, the link ds4 keeps to its current default model; the
`ds4flash` unit answers to `deepseek-v4-flash` whatever that link points
at. A model file named differently on your machine is one ARG at
install, for example
`DS41FLASH_Q2_MODEL=gguf/DeepSeek-V4.1-Flash-Q2.gguf`. Another model is
another unit.

A bee with no `DWARFSTAR_MODEL` serves every model it finds in
`DWARFSTAR_DIR` and its `gguf/`, skipping vision encoders, DSpark
support files and split pieces. Each answers to its family
(`deepseek-v4-flash`, `deepseek-v4.1-flash`, `deepseek-v4-pro`,
`glm-5.3-flash`, `glm-5.3`, `glm-5.2`, `qwen3.8-flash-next`) and, when
the file name says its quantisation, to the family with it (`-q2`,
`-q4`, `-q2-q4`, `-mxfp4`, `-fp8`). A pinned bee with no
`DWARFSTAR_MODELS` takes the same ids from its file's name. A prompt
that names no model gets the pinned file, or else `ds4flash.gguf` when
there is one, or else the first model found.

Each unit carries its resources in the Orch spec: `MEMORY` (the model's
resident size with room for its KV cache), `nice -n 10`, SSD streaming
on, eight engine threads, `RESTART on-failure` with a start limit of
three in a minute, and a stop timeout. launchd records `MEMORY` as a
soft resident-set limit that macOS does not enforce; systemd enforces it
as `MemoryMax`. On a Mac, what keeps memory in hand is that one model is
on the GPU at a time.

## One GPU, handed between bees

A bee loads its model only when a prompt for it arrives. ds4 exits when
a second process opens the GPU, so the bees take a hive-wide lock first
(`DWARFSTAR_GPU_LOCK`, which the Orchfile puts at
`$XDG_STATE_HOME/hum/hives/dwarfstar/gpu.lock`) and hold it for as long
as their engine is loaded:

- A bee that needs the GPU while another holds it files a want beside the
  lock and waits up to `DWARFSTAR_WAIT_MS`; past that the bloom breaks
  off with `gpu_busy`. A cancel ends the wait.
- The holder gives the GPU back when it is idle and someone wants it,
  after its current bloom when a want has waited a minute, and on its own
  after `DWARFSTAR_IDLE_MS` idle.
- Loading also checks ds4's own lock (`DS4_LOCK_FILE`), so a ds4 started
  outside the hive breaks the bloom off with `engine_unavailable`
  instead of killing the bee.
- Each bee claims headroom of one slot: free while the GPU is its own or
  free, none while another model's bee holds it. A bloom in flight does
  not take the slot; prompts queue behind it. humd sends a prompt to a
  nurse with room, here or on a trusted peer, before one without, so a
  prompt goes to a model that can start at once when one grows it; with none,
  it comes here and waits for the handoff.
- A bee pulses `cell-spawned` when it loads its model, `cell-evicted`
  when it gives the GPU back and `cell-died` when a load fails. Between
  blooms it pulses `cell-idle` only when its claim changes.

## How a bloom grows

- The engine keeps one session while it is loaded. Every bloom syncs it
  to the bloom's whole prompt, and ds4 evaluates only what the KV cache
  does not already hold.
- The bee is stateless: a prompt's `graft` is the turns before it (the
  text petals of each bloom's `content` as the user's turn, of its
  `grew` as the model's), and its own text petals, else its `text`, the
  turn it asks for. `systemPrompt` is the system turn. Other petals are
  dropped.
- A prompt for a model the bee does not answer to is refused with
  `no_such_model`, one with no words with `empty-prompt`, and one whose
  only words are in its graft with `cannot-grow-on`, since the engine
  grows only from new words. A refusal is an `error` with no wilt.
- Blooms grow one at a time, in order. A `cancel` stops its sid's bloom,
  waiting or in flight, and drops its sid's queued blooms, each wilting
  `cancel`.
- Thinking is off, and every bloom samples at `DWARFSTAR_TEMPERATURE`
  with ds4's own top-p and min-p, up to `DWARFSTAR_MAX_TOKENS`. A
  prompt's own `temperature`, `prune`, `season`, `frost`, `seed`,
  `trellis`, `sepal`, `scion` and `tools` are not read.
- The answer unfurls as one `text` petal. An unfurl carries whole UTF-8
  characters, even when a token ends inside one.
- A bloom wilts with `stop`, `max_tokens` (`DWARFSTAR_MAX_TOKENS` or
  the context is full), `cancel`, or `error` after an `error` tone
  (`gpu_busy`, `engine_unavailable`, `generation_failed`), and its
  nectar is the tokens in and out.
- The bee outlives humd: when humd restarts, it reconnects and nestles
  again with its model still loaded, dropping the blooms the old humd
  gave. Only a refusal, or its link failing, ends the bee, and its unit
  restarts it.

## Build

```sh
make                     # fetch the pinned ds4 source if needed, then build out/dwarfstar-bee
make DS4_DIR=~/src/ds4   # build against a ds4 checkout you already have
make check               # out/dwarfstar-bee-stub: the bee with no engine, on any machine
make fetch               # just fetch the pinned ds4 source into vendor/ds4
make clean               # drop out/
```

`install` runs `build`, which runs `make`. The engine objects are
exactly the ones ds4's own Makefile links on macOS, compiled with its
flags, so the numerics match upstream. The `.metal` kernels are copied
beside the bee, which points ds4 at them, so the bee runs from any
directory. The stub runs the thrum side and the GPU handoff with no
model: several stub bees on one lock trade the GPU the way the real ones
do.

## Environment

| name | meaning | default |
|---|---|---|
| `DWARFSTAR_MODEL` | pin the bee to this GGUF, absolute or under `DWARFSTAR_DIR` | every model in `DWARFSTAR_DIR` and its `gguf/` |
| `DWARFSTAR_MODELS` | the ids a pinned bee answers to, comma separated | taken from the file name |
| `DWARFSTAR_LABEL` | the label the bee enters as | `main` |
| `DWARFSTAR_DIR` | where the GGUFs live | `~/ds4` |
| `DWARFSTAR_CTX` | context size in tokens | `32768` |
| `DWARFSTAR_MAX_TOKENS` | cap on one bloom | `4096` |
| `DWARFSTAR_TEMPERATURE` | sampling temperature | ds4's, `1.0` |
| `DWARFSTAR_THREADS` | engine CPU threads | ds4's |
| `DWARFSTAR_SSD_STREAMING` | stream experts from SSD, unless `0` | off |
| `DWARFSTAR_IDLE_MS` | give the GPU back after this long idle | `300000` |
| `DWARFSTAR_WAIT_MS` | how long a bloom waits for the GPU | `300000` |
| `DWARFSTAR_GPU_LOCK` | the hive-wide GPU lock | none: the bee exits without it |
| `DWARFSTAR_METAL_DIR` | the `.metal` kernels | `metal/` beside the bee |
| `DS4_LOCK_FILE` | ds4's own lock | `/tmp/ds4.lock` |

These are the bee's defaults. The Orchfile sets each unit's label, model
and ids, `DWARFSTAR_SSD_STREAMING=1` and `DWARFSTAR_GPU_LOCK`, and its
ARGs `DWARFSTAR_DIR`, `DWARFSTAR_CTX`, `DWARFSTAR_MAX_TOKENS`,
`DWARFSTAR_IDLE_MS`, `DWARFSTAR_THREADS` (`8` there), `DS4_LOCK_FILE`
and the four model files can be set in the environment of
`hum hive ./hives/dwarfstar install`. The rest keep the bee's defaults
under the hive's units.

## What is not here

- **Splitting a model across machines.** ds4 can run one model across
  machines; hum's part would be finding the bee that holds the missing
  layers and vouching for it, which needs a capability claim with proof
  of possession.
- **Two models resident at once.** The handoff assumes one model fits;
  a machine that holds two would need the lock to count, not exclude.
- **Vision, tools and thinking.** The engine has them; the bee does not
  use them.
- **CUDA and ROCm.** ds4 builds for both; this Makefile wires Metal only.

## Licence

The engine is MIT, under the notices in its `LICENSE`: the ds4.c
authors, DeepSeek, and the ggml authors, for the quantisation formats
and kernels it adapts from llama.cpp and GGML. The `LICENSE` travels with
the fetched source.

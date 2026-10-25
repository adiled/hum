---
title: "dwarfstar"
description: "The DwarfStar engine linked into hum nurses, one bee per model, sharing one GPU"
---

# dwarfstar

> _Nurses with the DwarfStar engine inside them, one per model. A prompt tone goes into the model's token stream, and the answer unfurls back as a bloom._

[DwarfStar](https://github.com/antirez/ds4) (ds4) is a narrow, fast
local inference engine for a few open-weight models: DeepSeek V4 and
V4.1 Flash, GLM 5.2 and 5.3, Qwen3.8 Flash Next and DeepSeek V4 PRO. It
is a C library with a CLI, a server and an agent on top.

This hive takes the library, not the server. The bee compiles the engine
from a pinned upstream commit and links it in, and speaks thrum through
the [C client](../../thrum-clients/c). Nothing is serialised to HTTP and
back: the engine's session, KV cache and token stream are the bee's own.

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
| `ds4flash` | `DS4FLASH_MODEL`, ds4's own default link `ds4flash.gguf` | `deepseek-v4-flash` |
| `ds4flash-q2` | `DS4FLASH_Q2_MODEL` | `deepseek-v4-flash-q2` |
| `ds41flash-q2` | `DS41FLASH_Q2_MODEL` | `deepseek-v4.1-flash`, `deepseek-v4.1-flash-q2` |
| `glm53flash-q2` | `GLM53FLASH_Q2_MODEL` | `glm-5.3-flash`, `glm-5.3-flash-q2` |

Every unit runs the same `dwarfstar-bee`, pinned with `DWARFSTAR_MODEL`
and `DWARFSTAR_MODELS`, so each model is its own participant: its own
key, its own ids in its hello, its own headroom, and `hum bee <id> exit`
takes one model off the mesh and leaves the rest. A model file named
differently on your machine is one ARG at install, for example
`DS41FLASH_Q2_MODEL=gguf/DeepSeek-V4.1-Flash-Q2.gguf`. Another model is
another unit.

Each unit carries its resources in the Orch spec: `MEMORY` (the model's
resident size with room for its KV cache), `nice -n 10`, SSD streaming
and threads as ds4-runner set them, `RESTART on-failure` with a start
limit, and a stop timeout. launchd records `MEMORY` as a soft
resident-set limit that macOS does not enforce; systemd enforces it as
`MemoryMax`. On a Mac, what keeps memory in hand is that one model is on
the GPU at a time.

## One GPU, handed between bees

A bee loads its model only when a prompt for it arrives. ds4 exits when
a second process opens the GPU, so the bees take a hive-wide lock first
(`$XDG_STATE_HOME/hum/hives/dwarfstar/gpu.lock`) and hold it for as long
as their engine is loaded:

- A bee that needs the GPU while another holds it files a want beside the
  lock and waits up to `DWARFSTAR_WAIT_MS`. A cancel ends the wait.
- The holder gives the GPU back when it is idle and someone wants it,
  after its current bloom when a want has waited a minute, and on its own
  after `DWARFSTAR_IDLE_MS` idle.
- Loading also checks ds4's own lock (`DS4_LOCK_FILE`), so a ds4 started
  outside the hive is an error on the bloom, not a dead bee.
- Each bee claims one slot of headroom while the GPU is its own or free,
  and none while another model holds it, so a peer routes to the model
  that can start now. A local prompt still runs; it waits for the
  handoff. The claim changes with a `pulse` only when it changes.

## How a bloom grows

- The engine keeps one session while it is loaded. Every bloom syncs it
  to the bloom's whole prompt, and ds4 evaluates only what the KV cache
  does not already hold.
- Blooms grow one at a time, in order. A `cancel` stops its sid's bloom,
  waiting or in flight, and drops its sid's queued blooms, each wilting
  `cancelled`.
- The answer unfurls as one `text` petal. A bloom wilts with `stop`,
  `length` (`DWARFSTAR_MAX_TOKENS` or the context is full), `cancelled`,
  or `error` after an `error` tone, and its nectar is the tokens in and
  out.
- An unfurl carries whole UTF-8 characters, even when a token ends inside
  one.
- The bee outlives humd: when humd restarts, it reconnects and nestles
  again with its model still loaded, dropping the blooms the old humd gave.
- The bee is stateless: a prompt's text petals, else its `text` field,
  are the conversation. Petals with roles become chat messages.

## Build

```sh
make                     # fetch the pinned ds4 source if needed, then build out/dwarfstar-bee
make DS4_DIR=~/src/ds4   # build against a ds4 checkout you already have
make check               # out/dwarfstar-bee-stub: the bee with no engine, on any machine
```

The engine objects are exactly the ones ds4's own Makefile links on
macOS, compiled with its flags, so the numerics match upstream. The
`.metal` kernels are copied beside the bee, which points ds4 at them, so
the bee runs from any directory. The stub runs the thrum side and the GPU
handoff with no model: several stub bees on one lock trade the GPU the
way the real ones do.

## Environment

| name | meaning | default |
|---|---|---|
| `DWARFSTAR_MODEL` | pin the bee to this GGUF, absolute or under `DWARFSTAR_DIR` | every model under `DWARFSTAR_DIR` |
| `DWARFSTAR_MODELS` | the ids a pinned bee answers to, comma separated | earned from the file name |
| `DWARFSTAR_LABEL` | the label the bee enters as | `main` |
| `DWARFSTAR_DIR` | where the GGUFs live | `~/ds4` |
| `DWARFSTAR_CTX` | context size in tokens | `32768` |
| `DWARFSTAR_MAX_TOKENS` | cap on one bloom | `4096` |
| `DWARFSTAR_TEMPERATURE` | sampling temperature | ds4's default |
| `DWARFSTAR_THREADS` | engine CPU threads | ds4's default |
| `DWARFSTAR_SSD_STREAMING` | `1` streams experts from SSD | off |
| `DWARFSTAR_IDLE_MS` | give the GPU back after this long idle | `300000` |
| `DWARFSTAR_WAIT_MS` | how long a bloom waits for the GPU | `300000` |
| `DWARFSTAR_GPU_LOCK` | the hive-wide GPU lock | under the hive's state dir |
| `DWARFSTAR_METAL_DIR` | the `.metal` kernels | `metal/` beside the bee |
| `DS4_LOCK_FILE` | ds4's own lock | `/tmp/ds4.lock` |

## What is not here

- **The lattice.** ds4 can split a model across machines; hum's part
  would be finding the bee that holds the missing layers and vouching
  for it, which needs a capability claim with proof of possession.
- **Two models resident at once.** The handoff assumes one model fits;
  a machine that holds two would need the lock to count, not exclude.
- **Vision, tools and thinking.** The engine has them; the bee does not
  use them yet.
- **CUDA.** ds4 builds for CUDA on Linux; this Makefile wires Metal only.

## Licence

The engine is MIT, © Salvatore Sanfilippo, and credits llama.cpp and
GGML for the quantisation formats and kernels it inherits. Its
`LICENSE` travels with the fetched source.

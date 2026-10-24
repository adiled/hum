---
title: "dwarfstar"
description: "The DwarfStar engine linked into a hum worker bee, with no HTTP in between"
---

# dwarfstar

> _A worker bee with the DwarfStar engine inside it. A prompt tone goes into the model's token stream, and the answer comes back as chunk tones._

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
brew install pkgconf libsodium cjson
DWARFSTAR_DIR=~/ds4 hum hive ./hives/dwarfstar install
```

`install` builds the bee and runs it under orchd in the `hum-dwarfstar`
namespace. The bee enters itself under the hive as `main` when it starts
and signs its hello with that key. It builds on macOS, where ds4's Metal
backend is what it links.

## Build

```sh
make                     # fetch the pinned ds4 source if needed, then build out/dwarfstar-bee
make DS4_DIR=~/src/ds4   # build against a ds4 checkout you already have
make check               # out/dwarfstar-bee-stub: the bee with no engine, on any machine
```

The engine objects are exactly the ones ds4's own Makefile links on
macOS, compiled with its flags, so the numerics match upstream. The
`.metal` kernels are copied beside the bee, which points ds4 at them, so
the bee runs from any directory.

## Models

The bee looks in `DWARFSTAR_DIR` and its `gguf/` directory for the GGUFs
ds4's download script leaves there, and advertises an id for each:

| id | |
|---|---|
| `deepseek-v4-flash`, `deepseek-v4.1-flash`, `deepseek-v4-pro` | the family |
| `glm-5.3-flash`, `glm-5.3`, `glm-5.2`, `qwen3.8-flash-next` | the family |
| `<family>-q2`, `-q4`, `-q2-q4`, `-mxfp4`, `-fp8` | that quantisation, when the file name says |

humd routes a prompt to the bee by its `modelId`. Vision encoders, DSpark
support files and split layer pieces are not models and are not
advertised.

## How a turn runs

- One engine is resident at a time, since a model is tens of gigabytes;
  a prompt for another model closes the loaded one first. The engine is
  loaded on the first prompt that needs it.
- The engine keeps one session while it is loaded. Every turn syncs it
  to the turn's whole prompt, and ds4 evaluates only what the KV cache
  does not already hold.
- Turns run one at a time, in order. A `cancel` stops its sid's turn in
  flight and drops its sid's queued turns, each answered with
  `finish` `cancelled`.
- A turn ends in `finish` with `stop`, `length` (`DWARFSTAR_MAX_TOKENS`
  or the context is full), `cancelled`, or `error` after an `error` tone.
- Chunks carry whole UTF-8 characters, even when a token ends inside one.
- The bee is stateless: a prompt's `text`, or its `content` petals, are
  the conversation. Petals with roles become chat turns.

## Hello

| | |
|---|---|
| `roles` | `["worker"]` |
| `models` | the ids above |
| `chis` | `hello`, `prompt`, `cancel`, `chunk`, `finish`, `error` |
| `propensity` | stateless, lean, `dwarfstar/core` |
| `headroom` | one slot: one engine |

## Environment

| name | meaning | default |
|---|---|---|
| `DWARFSTAR_DIR` | where the GGUFs live | `~/ds4` |
| `DWARFSTAR_CTX` | context size in tokens | `32768` |
| `DWARFSTAR_MAX_TOKENS` | cap on one turn | `4096` |
| `DWARFSTAR_TEMPERATURE` | sampling temperature | ds4's default |
| `DWARFSTAR_METAL_DIR` | the `.metal` kernels | `metal/` beside the bee |
| `DWARFSTAR_LABEL` | the label the bee enters as | `main` |

`DWARFSTAR_DIR`, `DWARFSTAR_CTX` and `DWARFSTAR_MAX_TOKENS` are Orchfile
ARGs, so `install` passes them through from its environment.

## What is not here

- **The lattice.** ds4 can split a model across machines; hum's part
  would be finding the bee that holds the missing layers and vouching
  for it, which needs a capability claim with proof of possession.
- **Vision, tools and thinking.** The engine has them; the bee does not
  use them yet.
- **CUDA.** ds4 builds for CUDA on Linux; this Makefile wires Metal only.

## Licence

The engine is MIT, © Salvatore Sanfilippo, and credits llama.cpp and
GGML for the quantisation formats and kernels it inherits. Its
`LICENSE` travels with the fetched source.

---
title: "dwarfstar"
description: "The DwarfStar engine, embedded — ds4 as a hum cell, with no HTTP in between"
---

# dwarfstar

> _A worker bee that links the DwarfStar engine into its own process. A prompt tone goes straight into the model's token stream, and the answer comes back as chunk tones. No HTTP, no subprocess, no adapter._

DwarfStar ([antirez/ds4](https://github.com/antirez/ds4)) is a narrow,
fast local inference engine for a few excellent open-weight models:
DeepSeek V4 and V4.1 Flash, GLM 5.2 and 5.3, Qwen3.8 Flash Next, and
DeepSeek V4 PRO. It is a C library with a CLI, a server, and an agent on
top of it.

This hive takes the library, not the server. The bee compiles the engine
from the pinned upstream source and links it in, so the model runs
*inside* the bee. That is the difference from a forager that points at
`ds4-server`: nothing is serialised, nothing is re-parsed, and the
session's KV state, the vision encoder, the disk cache, and the token
stream are all the engine's own objects.

```
nestler                      dwarfstar-bee                    humd
   │                             │                             │
   │  chi:"prompt"               │                             │
   ├────────────────────────────►│  chi:"prompt"               │
   │                             ├────────────────────────────►│
   │                             │                             │ routes by model id
   │                             │  ds4 session: sync, sample, │
   │                             │  eval, detokenize           │
   │  chi:"chunk" (text)         │                             │
   │◄────────────────────────────┤                             │
   │  chi:"finish"               │                             │
   │◄────────────────────────────┤                             │
```

## Propensity

| statefulness | richness | wire shape | hides |
|---|---|---|---|
| stateful | medium | `dwarfstar/core` | pulse, breath, drone, perf-mark, tendril, permission-ask |

The engine holds a live KV cache and a disk-resident prefix cache, so a
session continues where it left off. The bee advertises the models it
found on disk, and humd routes a prompt to it by model id.

## Models

The bee looks in `DWARFSTAR_DIR` (default `~/ds4`) for the GGUFs ds4's
download script left there, and gives each one an id:

| id | file |
|---|---|
| `deepseek-v4-flash` | the representative DeepSeek V4 Flash artifact |
| `deepseek-v4-flash-q4` / `-q2` / `-mxfp4` | that quantisation |
| `deepseek-v4.1-flash` | DeepSeek V4.1 Flash |
| `glm-5.3-flash` / `-q4` / `-q2` / `-fp8` | GLM 5.3 Flash |
| `glm-5.2` | GLM 5.2 |
| `qwen3.8-flash-next` | Qwen3.8 Flash Next |
| `deepseek-v4-pro` | DeepSeek V4 PRO |

Vision encoders, DSpark support files, and split layer pieces are not
models and are not advertised.

## Build

The engine source is fetched at a pinned commit and compiled into the
bee. No git submodule, no vendored megabytes in the tree, no shared
library, no ABI.

```sh
make                 # fetch the pinned ds4 source if needed, then build
make DS4_DIR=~/src/ds4   # use a checkout you already have
```

The build uses ds4's own compiler flags so the numerics match upstream.

## Install

```sh
hum hive "$PWD/hives/dwarfstar" install
```

That builds the hive, installs `dwarfstar-bee` into `~/.local/bin`,
copies the Metal kernels to `~/.local/share/hum/dwarfstar/metal`, and
registers the bee with humd through the Orchfile.

## Environment

| name | meaning | default |
|---|---|---|
| `DWARFSTAR_DIR` | where the GGUFs live | `~/ds4` |
| `DWARFSTAR_CTX` | context size | `32768` |
| `DWARFSTAR_MAX_TOKENS` | cap on one turn | `4096` |
| `DWARFSTAR_TEMPERATURE` | sampling temperature | `1.0` |
| `DWARFSTAR_METAL_DIR` | where the `.metal` kernels live | next to the binary |

## Licence

The engine is MIT, © Salvatore Sanfilippo. ds4 itself credits llama.cpp
and GGML for the quantisation formats and kernels it inherits; the
upstream `LICENSE` travels with the fetched source.

/* engine — the process a dwarfstar bee stands for.
 *
 * DwarfStar is antirez/ds4: a narrow local inference engine for a few
 * excellent open-weight models. It is a C library with a CLI, a server,
 * and an agent on top of it. This hive links the library into the bee
 * rather than driving the server over HTTP, so the process and the
 * participant are the same process here — but the bee is still the one
 * that speaks thrum and answers humd.
 *
 * The engine's material lives outside the hive:
 *
 *   the source   fetched at the pinned commit, into vendor/ds4
 *   the weights  wherever ds4's download script left them
 *
 * The bee finds the weights itself (DWARFSTAR_DIR) and advertises the
 * models it finds, so humd routes a prompt to this bee by model id.
 *
 * The pin: antirez/ds4 @ 5379c1c9ac315f1540371b06a3db88491d4059ff
 */

# dwarfstar engine

The process behind the bee. Three things, none of them in the hive's
directory:

| what | where | how |
|---|---|---|
| the source | `vendor/ds4` | fetched at the pinned commit by the build |
| the weights | `DWARFSTAR_DIR` | where ds4's `download_model.sh` left them |
| the kernels | `out/metal` | beside the bee, found at startup |

## The models

The bee looks in `DWARFSTAR_DIR` and in the `gguf/` directory under it,
and gives each artifact an id:

| id | artifact |
|---|---|
| `deepseek-v4-flash` | the representative DeepSeek V4 Flash |
| `deepseek-v4-flash-q2` | the pure Q2 quantisation |
| `deepseek-v4-flash-q2-q4` | the mixed quantisation — Q4 experts, Q2 elsewhere |
| `deepseek-v4.1-flash` | DeepSeek V4.1 Flash |
| `glm-5.3-flash` | GLM 5.3 Flash |
| `deepseek-v4-pro` | DeepSeek V4 PRO, split across two machines |

Vision encoders, DSpark support files, and split layer pieces are not
models and are not advertised.

## The GPU

One checkpoint at a time. The weights are 81 GB and upward; a 128 GB
machine holds one of them resident. So the archetypes in the Orchfile are
alternatives, not a set: enter the one you want, and the bee that serves
it holds the model.

That is the same reason the ds4-runner has an arbiter — five checkpoints
share one GPU behind one address, and something has to decide who holds
it. Here the decision is the Orchfile: each archetype is a service, and
you enter one.

## Licence

MIT, © Salvatore Sanfilippo. ds4 itself credits llama.cpp and GGML for
the quantisation formats and kernels it inherits; the upstream `LICENSE`
travels with the fetched source.

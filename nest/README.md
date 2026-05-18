---
title: "nest"
description: "the place inside humd where bees gather and roosts live — defines the WorkerBee + ForagerBee traits, the Roost runtime, and the Nest pool"
---

# nest

> _the place inside humd where bees gather and roosts live — defines
> the `WorkerBee` + `ForagerBee` traits, the `Roost` runtime, and the
> `Nest` pool_

A **nest** is the space inside a humd where two kinds of inhabitants
meet: **bees** (the askers / producers, connected over thrum) and
**roosts** (the live LLM subprocesses, spawned by worker bees). The
nest itself isn't a process or a model — it's the meeting place. One
humd, one nest.

This crate defines what lives in the nest and how:

- **`WorkerBee`** trait — produces compute. Owns the spawn, the stdin
  pipe, the parsed-event stream, and the roost lifecycle.
- **`ForagerBee`** trait — translates outside wires (OpenAI, Anthropic,
  custom HTTP) into thrum. Stub-only today; concrete impls live in
  [`hives/`](../hives).
- **`Roost`** struct — one live LLM subprocess. The compute itself.
- **`Listener`** trait — what humd binds to a roost to receive parsed
  events for a particular sid.
- **`Nest`** struct — the pool runtime. Owns the roosts keyed by
  pool_key, dispatches stdin writes, evicts on idle, enforces
  `max_procs`.

humd today is a router: it doesn't link `WorkerBee` impls in at build
time. Instead, worker-bee binaries (under [`hives/`](../hives)) attach
to humd over thrum and announce themselves with `chi:"hello"` carrying
`bee: ["worker"]`. The `Nest` pool above is the Rust SDK for in-process
embeddings — the canonical wire-attached path is the helper in
[`hives/common/`](../hives/common) (`nest_common::serve_worker`).

## The three things — clearly

There are exactly three things this crate is about. They get confused
constantly. The README will spell them out and then never let them blur:

| word | what it is | example |
|---|---|---|
| **nest** | the space inside humd. Not a process, not a model. Where bees and roosts coexist. | one per humd |
| **roost** | one live LLM subprocess that lives in the nest. The actual compute. | the claude-cli child process |
| **WorkerBee** (Rust trait) | the *kind* of compute — how to spawn it, what it speaks, when it's ephemeral. Implementation. | `claude-cli`, `claude-repl`, future `openai-api` |

And the cohabitants from the other side of the thrum:

| word | what it is |
|---|---|
| **hive** | the kind/contract a bee conforms to (defined in [`hives/`](../hives) or [`hives/`](../hives)). Doesn't run; doesn't send anything. |
| **bee** | the running instance. Declares its kinds on hello: `bee: ["worker"]`, `bee: ["forager"]`, or both. |
| **nestler** | the bee, *in the act of joining*. Pre-acceptance. Awaiting the breath. |
| **nestled** | the bee, *after joining*. Same actor, registered, has a nestledId. Keeps asking throughout the connection. |

Seven words, seven different referents. nestler → nestled is a state
transition (one actor, two lifecycle stages); both are askers.

## Where the nest fits

```
bees (outside)
   │   thrum tones (NDJSON over Unix socket)
   ▼
humd
   │
   ├── ensemble        ← cross-humd routing
   │
   └── nest            ← THE SPACE (this crate)
        │
        ├── nestled<1>  ← bees, post-handshake
        ├── nestled<2>
        │
        └── roosts      ← live LLM subprocesses
             │
             ├── roost<A> spawned by claude-cli WorkerBee
             ├── roost<B> spawned by claude-repl WorkerBee
             └── roost<C> spawned by openai-api WorkerBee (hypothetical future)
```

The nest is BELOW ensemble (ensemble routes between humds; nest is
local to one humd) and INSIDE humd. Nestleds and roosts share it.
chi traffic crosses between them: a nestled's `chi:"prompt"` is routed
to a worker bee's roost; that roost's chunks flow back to the nestled.

## The `WorkerBee` trait

```rust
#[async_trait]
pub trait WorkerBee: Send + Sync {
    fn ephemeral(&self) -> bool;
    async fn spawn(&self, spec: SpawnSpec) -> Result<Roost>;
}
```

A `WorkerBee` is a *recipe* for a kind of roost. Two methods:

- `ephemeral()` — does the pool evict this roost after each `result`?
  (PTY/REPL-style harnesses say yes; pipe-mode say no.)
- `spawn(spec)` — turn a high-level `SpawnSpec` (sid, modelId, cwd,
  system prompt, MCP url, …) into a running `Roost`.

The trait says nothing about *what* the roost is. A WorkerBee impl
might fork a local Claude binary, open an HTTPS connection to OpenAI's
API, load a llama.cpp model into the same address space, or return a
canned-response mock for tests. The wire never sees the difference —
all the worker has to do is produce a `Roost` whose `stdin → events`
behaves correctly.

### Roost

```rust
pub struct Roost {
    pub pid: Option<u32>,
    pub stdin: mpsc::Sender<String>,            // push raw NDJSON to the roost
    pub events: Arc<Mutex<mpsc::Receiver<Value>>>,  // pull parsed JSON back
    pub exited: tokio::sync::oneshot::Receiver<i32>,
    pub ephemeral: bool,
    pub kill: Arc<dyn Fn() + Send + Sync>,
}
```

One live LLM subprocess seen from the humd side. The `stdin` and
`events` channels are the only contract — whatever's behind them is
the WorkerBee impl's business.

## The `Nest` pool

```rust
let nest = Nest::new(
    NestConfig { max_procs: 8, idle_timeout: Duration::from_secs(300) },
    pipe_worker,  // Arc<dyn WorkerBee> — long-lived pipe-mode roosts
    pty_worker,   // Arc<dyn WorkerBee> — ephemeral PTY/REPL roosts
);
```

Two worker bees by convention: a pipe-mode one and a PTY-mode one.
A pool that wants to colocate compute keeps these in-process. humd
today doesn't — it routes prompts to thrum-attached worker bees that
live in their own processes.

Operations the daemon calls on the pool when it's in use:

| call | what happens |
|---|---|
| `nest.murmur(spec, prompt, listener)` | spawn-if-needed + write `chi:"prompt"` to stdin + bind listener for the sid |
| `nest.reply(sid, tool_use_id, result)` | route a `chi:"tool-result"` reply to the right roost's stdin |
| `nest.interrupt(sid, request_id)` | inject `control_cancel_request` mid-turn |
| `nest.fell(sid)` | tear the roost down — host called `chi:"cleanup"` |

## Why a trait, not a hard-coded path

hum is LLM-agnostic by design. The protocol (thrum), the mesh
(ensemble), the drone, the bees — none of them know which model is
behind the nest. `WorkerBee` is the seam where that decision lives,
and where it gets swapped.

| kind of roost | what its worker spawns | when to use |
|---|---|---|
| `claude-cli` | `claude -p` with `stream-json` over pipe | normal model-CLI usage |
| `claude-repl` | claude-cli in interactive REPL mode over a PTY | non-stream-json fallbacks, debugging |
| (future) `openai-api` | HTTPS to OpenAI's API; chunks streamed via SSE | use GPT-4 from your humd |
| (future) `ollama-local` | local LLM via Ollama's CLI or HTTP | run open-weights models on a laptop |

A new model harness adds one `WorkerBee` impl. No code in `humd`,
`thrum-core`, `ensemble`, `drone`, or the wire needs to change.

## What this crate is NOT

- **Not "compute" by itself.** The compute is the **roost**. This
  crate is the *space* the roost lives in and the *traits* that define
  what kind of bee can produce / translate. Same nest word, distinct
  referents — nest is the room, roost is who lives there.
- **Not a thrum endpoint.** Roosts don't speak thrum directly. humd
  reads roost events via `Listener` and writes the thrum side. Worker
  bees running as standalone processes write thrum themselves via
  [`hives/common/serve_worker`](../hives/common).
- **Not a router.** `Nest` picks the right roost for a sid; it doesn't
  decide which humd handles which conversation. Ensemble owns that.

## Layout

```
nest/
├── src/
│   ├── lib.rs   # SpawnSpec, Roost, WorkerBee + ForagerBee traits,
│   │           # Listener trait, encode_* helpers
│   ├── pool.rs  # Nest — the roost pool runtime
│   └── mock.rs  # MockWorkerBee for sim tests
└── README.md
```

Concrete `WorkerBee` impls live under [`hives/`](../hives):

- [`hives/claude-cli/`](../hives/claude-cli) — `claude -p` stream-json over pipe
- [`hives/claude-repl/`](../hives/claude-repl) — PTY-mode interactive fallback
- [`hives/common/`](../hives/common) — shared building blocks across worker impls
  (e.g. the `serve_worker` helper + the regex `Classifier` for drone's
  context-loss detection)

## See also

- [WIRE.md](../WIRE.md) — the thrum protocol; see "The nest model" section
  for what the wire sees (and what it doesn't) about nests and roosts.
- [`drone/`](../drone) — the sentinel that observes the chunks coming
  out of a roost and scores channel health.
- [`humd/`](../humd) — the daemon that owns the `Nest` instance.
- [VOCABULARY](../VOCABULARY.md) — the canonical glossary; entries for
  nest, roost, hive, bee, worker, forager, nestler, nestled.

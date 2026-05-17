---
title: "nest"
description: "the place inside humd where nestlers nestle and roosts live — defines the Perch trait, the Roost runtime, and the Nest pool"
---

# nest

> _the place inside humd where nestlers nestle and roosts live — defines the `Perch` trait, the `Roost` runtime, and the `Nest` pool_

A **nest** is the space inside a humd where two kinds of inhabitants
meet: **nestlers** (the askers, connected over thrum) and **roosts**
(the live LLM subprocesses, spawned by humd). The nest itself
isn't a process or a model — it's the meeting place. One humd, one
nest.

This crate defines what lives in the nest and how:

- **`Perch`** trait — defines a *kind* of roost. Owns the spawn,
  the stdin pipe, the parsed-event stream, and the lifecycle.
- **`Roost`** struct — one live LLM subprocess. The compute itself.
- **`Listener`** trait — what humd binds to a roost to receive parsed
  events for a particular sid.
- **`Nest`** struct — the pool runtime. Owns the roosts keyed by
  pool_key, dispatches stdin writes, evicts on idle, enforces
  `max_procs`.

The daemon (`humd`) holds an `Arc<Nest>` and one or more `Arc<dyn
Perch>`. It doesn't know what an LLM is. It just calls
`nest.spawn(spec)` when a new conversation starts and reads events
off the resulting roost.

## The three things — clearly

There are exactly three things this crate is about. They get confused
constantly. The README will spell them out and then never let them blur:

| word | what it is | example |
|---|---|---|
| **nest** | the space inside humd. Not a process, not a model. Where nestlers and roosts coexist. | one per humd |
| **roost** | one live LLM subprocess that lives in the nest. The actual compute. | the claude-cli child process |
| **Perch** (Rust trait) | defines a *kind* of roost — how to spawn it, what it speaks, when it's ephemeral. Implementation. | `claude-cli`, `claude-repl`, future `openai-api` |

And the cohabitants from the other side of the thrum:

| word | what it is |
|---|---|
| **nestling** | the kind/typology a nestler conforms to (defined in [`nestlings/`](../nestlings)). Doesn't run; doesn't send anything. |
| **nestler** | the running instance, *pre-acceptance*. The live process sending its first ask (`chi:"hello"`). Awaits the breath. |
| **nestled** | the running instance, *post-acceptance*. Same actor, registered, has a nestledId. **Keeps asking throughout the connection** — prompts, cancels, tool-results, release-permits, cleanups. Hello is the first ask; everything else flows from the nestled state. |

Six words, six different referents. Nest is one of them. Roost is
another. They are not interchangeable. nestler → nestled is a state
transition (one actor, two lifecycle stages); both are askers.

## Where the nest fits

```
nestlers (outside)
   │   thrum tones (NDJSON over Unix socket)
   ▼
humd
   │
   ├── ensemble        ← cross-humd routing
   │
   └── nest            ← THE SPACE (this crate)
        │
        ├── nestled<1>  ← nestlers, post-handshake
        ├── nestled<2>
        │
        └── roosts      ← live LLM subprocesses
             │
             ├── roost<A> spawned by claude-cli Perch
             ├── roost<B> spawned by claude-repl Perch
             └── roost<C> spawned by openai-api Perch (hypothetical future)
```

The nest is BELOW ensemble (ensemble routes between humds; nest is
local to one humd) and INSIDE humd. Nestleds and roosts share it.
chi traffic crosses between them: a nestled's `chi:"prompt"` is routed
to a roost; that roost's chunks flow back to the nestled.

## The `Perch` trait

```rust
#[async_trait]
pub trait Perch: Send + Sync {
    fn ephemeral(&self) -> bool;
    async fn spawn(&self, spec: SpawnSpec) -> Result<Roost>;
}
```

A `Perch` is a *recipe* for a kind of roost. Two methods:

- `ephemeral()` — does the pool evict this roost after each `result`?
  (PTY/REPL-style harnesses say yes; pipe-mode say no.)
- `spawn(spec)` — turn a high-level `SpawnSpec` (sid, modelId, cwd,
  system prompt, MCP url, …) into a running `Roost`.

The trait says nothing about *what* the roost is. A Perch impl might
fork a local Claude binary, open an HTTPS connection to OpenAI's API,
load a llama.cpp model into the same address space, or return a
canned-response mock for tests. The wire never sees the difference —
all the Perch has to do is produce a `Roost` whose `stdin → events`
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
the Perch impl's business.

## The `Nest` pool

```rust
let nest = Nest::new(
    NestConfig { max_procs: 8, idle_timeout: Duration::from_secs(300) },
    pipe_perch,  // Arc<dyn Perch> — long-lived pipe-mode roosts
    pty_perch,   // Arc<dyn Perch> — ephemeral PTY/REPL roosts
);
```

Two perches by convention: a pipe-mode one and a PTY-mode one.
humd's prompt router picks which to spawn into based on the conversation.

Operations the daemon calls on the pool:

| call | what happens |
|---|---|
| `nest.murmur(spec, prompt, listener)` | spawn-if-needed + write `chi:"prompt"` to stdin + bind listener for the sid |
| `nest.reply(sid, tool_use_id, result)` | route a `chi:"tool-result"` reply to the right roost's stdin |
| `nest.interrupt(sid, request_id)` | inject `control_cancel_request` mid-turn |
| `nest.evict(sid)` | tear the roost down — host called `chi:"cleanup"` |

## Why a trait, not a hard-coded path

hum is LLM-agnostic by design. The protocol (thrum), the mesh
(ensemble), the drone, the nestlings — none of them know which
model is behind the nest. `Perch` is the seam where that decision
lives, and where it gets swapped.

| kind of roost | what its Perch spawns | when to use |
|---|---|---|
| `claude-cli` | `claude -p` with `stream-json` over pipe | normal model-CLI usage |
| `claude-repl` | claude-cli in interactive REPL mode over a PTY | non-stream-json fallbacks, debugging |
| (future) `openai-api` | HTTPS to OpenAI's API; chunks streamed via SSE | use GPT-4 from your humd |
| (future) `ollama-local` | local LLM via Ollama's CLI or HTTP | run open-weights models on a laptop |

A new model harness adds one `Perch` impl and registers it with the
daemon. No code in `humd`, `thrum-core`, `ensemble`, `drone`, or
the wire needs to change.

## How the mesh learns what nests humds have

The wire doesn't advertise nests directly. The **humd** advertises its
configured nests in `PeerCapabilities.nests` (a `Vec<String>` of Perch
names), which the ensemble layer gossips to peers. A peer humd looking
to route a `chi:"prompt"` with `modelId: "gpt-4"` consults the
capability gossip, finds a humd that advertises `openai-api`, and
routes there.

This is the existing mechanism — nothing new needed for "non-repo-bounded
nests." Anyone who wants to add a new kind of compute:

1. Writes a `Perch` impl in their own Rust crate.
2. Builds it into their humd (or, future: dynamic plugin loading).
3. Runs the humd with that Perch registered.
4. The humd's existing capability gossip advertises the new nest name.
5. Other humds on the mesh discover and route to it.

The wire stays unchanged. The Perch lives in any repo. The mesh
sees one more capable peer.

## How ensemble uses the nest (and doesn't)

Ensemble doesn't import this crate. The mesh layer routes tones; it
has no opinion on whether a humd's nest is empty, full, local-only,
or pointed at a remote API. A humd with no nest is still a valid
ensemble peer — it can forward tones, gossip, run kad lookups — it
just can't *answer* a `chi:"prompt"` locally.

That's the value the nest brings to ensemble: it makes a humd a useful
**destination**. Without a nest, a humd is a relay. With one, it's a
correspondent.

Three concrete scenarios where the nest/ensemble separation pays off:

| scenario | what the nest does | what ensemble does |
|---|---|---|
| [overflow-inference](../scenarios/overflow-inference.md) | humd-A's nest reports its `max_procs` is full | ensemble routes the prompt to humd-B whose nest has a free slot |
| [phone-laptop-roam](../scenarios/phone-laptop-roam.md) | the new humd's nest resumes the conversation from its own transcript store | ensemble carries `chi:"prompt"` across devices and routes replies back |
| [federation-handoff](../scenarios/federation-handoff.md) | org-A's humd has a `claude-cli` perch; org-B's has a different one | ensemble passes prompts cross-org without either side needing to know what the other's nest looks like |

In every case: ensemble carries the *envelope*; the nest's roost
produces the *reply*. The interface between them is the chi vocabulary.
Nothing model-shaped crosses the seam.

## What this crate is NOT

- **Not "compute" by itself.** The compute is the **roost**. This
  crate is the *space* the roost lives in and the *trait* that defines
  what kind of roost it is. Same word, distinct referents — nest is
  the room, roost is who lives there.
- **Not a thrum endpoint.** Roosts don't speak thrum directly. humd
  reads roost events via `Listener` and writes the thrum side.
- **Not a router.** `Nest` picks the right roost for a sid; it doesn't
  decide which humd handles which conversation. Ensemble owns that.
- **Not a transcript store.** PipePerch hands events to the listener;
  persistence is the harness's problem (claude-cli's graft module
  writes JSONL).
- **Not a permissions enforcer.** The daemon's permit handler runs
  before a `chi:"tool-call"` ever lands here. Nest just sees what
  the model produced.

## Layout

```
nest/
├── src/
│   ├── lib.rs   # SpawnSpec, Roost, Perch trait, Listener trait, encode_* helpers
│   ├── pool.rs  # Nest — the roost pool runtime
│   └── mock.rs  # MockPerch for sim tests
└── README.md
```

Concrete `Perch` impls live under [`perches/`](../nests):

- [`perches/claude-cli/`](../perches/claude-cli) — `claude -p` stream-json over pipe
- [`perches/claude-repl/`](../perches/claude-repl) — PTY-mode interactive fallback
- [`perches/common/`](../perches/common) — shared building blocks across nest impls
  (e.g. the regex `Classifier` for drone's context-loss detection)

## See also

- [WIRE.md](../WIRE.md) — the thrum protocol; see "The nest model" section
  for what the wire sees (and what it doesn't) about nests and roosts.
- [`drone/`](../drone) — the sentinel that observes the chunks coming
  out of a roost and scores channel health.
- [`humd/`](../humd) — the daemon that owns the `Nest` instance.
- [VOCABULARY](../VOCABULARY.md) — the canonical glossary; entries for
  nest, roost, Perch, nestler, nestled, nestling.

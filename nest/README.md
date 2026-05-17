---
title: "nest"
description: "the model-process pool — `Perch` trait, `Roost` runtime, and the `Nest` that manages them"
---

# nest

> _the model-process pool — `Perch` trait, `Roost` runtime, and the `Nest` that manages them_

`nest` is the seam where humd meets an LLM process. It defines the
trait every model harness must implement (`Perch`), the shape of one
live model subprocess (`Roost`), the callback shape a daemon hooks
into a roost (`Listener`), and the pool runtime that owns it all
(`Nest`). Concrete nest implementations — `claude-cli`,
`claude-repl`, and any future kinds — live under
[`nests/`](../nests) and implement this trait.

The daemon (`humd`) holds an `Arc<dyn Perch>` and an
`Arc<Nest>`. It doesn't know what an LLM is. It just calls
`nest.spawn(spec)` when a new conversation starts and reads `Petal`
events off the roost's event channel.

## Where it fits

```
nestlers              ← humans, agents, plugins, HTTP frontends
   │   thrum tones (NDJSON over Unix socket)
   ▼
humd                  ← per-machine daemon: hums, MCP, drone
   │
   ├── ensemble       ← cross-humd routing (above)
   │
   └── nest           ← model-process pool  (this crate)
       │              │
       │              └── nest_common — shared building blocks
       │
       └── perches: claude-cli, claude-repl, future kinds
              │
              ▼
           the actual LLM process
```

**Nest sits BELOW ensemble, INSIDE humd.** Ensemble routes prompts
between humds; nest spawns the model that answers them. Two layers,
fully decoupled — neither imports the other. The daemon orchestrates
them both.

## How ensemble uses nest (and doesn't)

Ensemble doesn't import the `nest` crate. The mesh layer routes
*tones*; it has zero opinion about whether the tone's destination is
backed by a real model, a mock, or a different kind of agent entirely.
A humd without a nest is still a valid ensemble peer — it can
forward `chi:"prompt"` tones, run `kad_find`, gossip, advertise
nestlings. It just can't *answer* a prompt locally.

That's the actual value nest brings to ensemble: it makes a humd a
useful **destination**. Without nest, a mesh of humds is just a
post office relaying envelopes nobody opens. With nest, every humd
becomes a place a prompt can land and produce a real answer.

Three concrete scenarios where this layer separation matters:

| scenario | what nest does | what ensemble does |
|---|---|---|
| [overflow-inference](../scenarios/overflow-inference.md) | humd-A's nest reports its `max_procs` is full | ensemble routes the prompt to humd-B which still has a free roost |
| [phone-laptop-roam](../scenarios/phone-laptop-roam.md) | the new humd's nest resumes the conversation from its own transcript store (via the sigil) | ensemble carries the `chi:"prompt"` across devices and routes replies back |
| [federation-handoff](../scenarios/federation-handoff.md) | org-A's humd has a `claude-cli` perch; org-B's humd has a different perch (maybe a private model) | ensemble passes prompts cross-org without either side needing to know what model the other runs |

In every case, ensemble carries the *envelope*; nest produces the
*reply*. The interface between them is the chi vocabulary
(`chi:"prompt"` in, `chi:"chunk"`/`chi:"finish"` out) — nothing
model-shaped crosses the seam.

## Capability advertising

When humd negotiates with peer humds in the ensemble, it advertises
what its local nest can do via `PeerCapabilities.nests`. A humd
with a `claude-cli` perch installed advertises
`nests: ["claude-cli"]`. The overflow scenario uses this to find a
peer that can actually run the requested model:

```rust
let target = ensemble.pick_peer_with_nest("claude-cli");
ensemble.route(prompt.to_humd(target)).await?;
```

Without nest's advertise, ensemble would have no idea which peer can
serve which prompt. Without ensemble's routing, nest would be
single-machine only. Together they make the four-tier (T1 own-devices
→ T4 open mesh) story work end-to-end.

## Concepts

| word | what it means |
|---|---|
| **nest** | the *kind* of model harness — a `Perch` impl (claude-cli, claude-repl, future) |
| **Perch** | the trait. Tells the daemon "give me a `SpawnSpec`, I'll hand back a live `Roost`" |
| **Roost** | one live model subprocess. Has stdin / event-stream / kill / exit-code |
| **Listener** | callback shape the daemon binds to a roost — receives parsed petals |
| **petal** | one parsed event off the model's stdout (text chunk, tool_use, finish, etc) |
| **Nest** | the pool. Owns roosts keyed by `pool_key`, dispatches stdin writes, evicts on idle, enforces `max_procs` |
| **SpawnSpec** | the high-level recipe the daemon hands a perch (sid, model, cwd, system prompt, MCP url, …) |
| **pool_key** | typically `sid` — the key under which a roost lives in the nest. One roost per key. |

## The trait

```rust
#[async_trait]
pub trait Perch: Send + Sync {
    fn ephemeral(&self) -> bool;
    async fn spawn(&self, spec: SpawnSpec) -> Result<Roost>;
}
```

That's it. Two methods. `ephemeral()` flags whether the pool should
evict the roost after each `result` (PTY/REPL-style harnesses); the
spawn method turns a `SpawnSpec` into a running process.

The `Roost` returned by `spawn` exposes:
- `stdin: mpsc::Sender<String>` — push raw NDJSON lines into the
  child's stdin
- `events: Arc<Mutex<mpsc::Receiver<Value>>>` — pull parsed JSON
  events off the child's stdout
- `exited` — a oneshot that resolves with the child's exit code
- `kill` — best-effort kill the child

Stream parsing lives in `pool.rs`. The pool watches `events`,
classifies each frame, and dispatches typed callbacks
(`on_petal`/`on_roost`/`on_wilt`/`on_thorn`) to whichever
`Listener` is registered for that roost's active sid.

## The pool

```rust
let nest = Nest::new(
    NestConfig { max_procs: 8, idle_timeout: Duration::from_secs(300) },
    pipe_perch,  // Arc<dyn Perch> — long-lived pipe-mode roosts
    pty_perch,   // Arc<dyn Perch> — ephemeral PTY/REPL roosts
);
```

Two perches by convention: one for **pipe-mode** roosts (long-lived,
stream-json over stdin/stdout — the canonical Claude CLI shape) and
one for **PTY-mode** roosts (ephemeral, evicted on each turn —
REPL-style harnesses). The daemon picks which to spawn into based on
the conversation's needs.

Operations the daemon calls on the pool:

| call | what happens |
|---|---|
| `nest.murmur(spec, prompt, listener)` | spawn-if-needed + write a `chi:"prompt"` to stdin + bind the listener for the sid |
| `nest.reply(sid, tool_use_id, result)` | route a `chi:"tool-result"` reply to the right roost's stdin |
| `nest.interrupt(sid, request_id)` | inject `control_cancel_request` mid-turn |
| `nest.evict(sid)` | tear the roost down — host called `chi:"cleanup"` |

## Why a trait, not a hard-coded Claude path

hum is LLM-agnostic by design. The protocol (thrum), the mesh
(ensemble), the drone, the nestlings — none of them know which
model is behind the nest. `Perch` is where that decision lives, and
where it gets swapped:

| nest kind | what it spawns | when to use |
|---|---|---|
| `claude-cli` | `claude -p` with `stream-json` over pipe | normal model-CLI usage |
| `claude-repl` | claude-cli in interactive REPL mode over a PTY | non-stream-json fallbacks, debugging |
| (future) `gemini-cli` | `gemini` CLI similarly | swap LLM vendor |
| (future) `ollama-local` | local LLM through Ollama's CLI | local-LLM-only setups |

A new model harness adds one `Perch` impl and registers it with the
daemon. No code in `humd`, `thrum-core`, `ensemble`, or `drone`
needs to change.

## SpawnSpec

The recipe the daemon hands a perch. Harness-agnostic — the perch
translates these into whatever its underlying tool needs (claude CLI
flags, ollama args, env vars, etc).

```rust
pub struct SpawnSpec {
    pub sid: String,                 // hum session id; usually the pool_key
    pub model_id: String,            // "claude-sonnet-4-6", "gemini-pro", ...
    pub cwd: String,                 // working directory for the spawned process
    pub system_prompt: Option<String>,
    pub mcp_url: Option<String>,     // hum's MCP HTTP base — wires tool surface in
    pub cli_path: Option<String>,    // override the binary location
    pub resume_id: Option<String>,   // pick up an existing transcript
    pub plan_mode: bool,
    pub permissions: Vec<String>,    // tool allowlist by name
    pub allowed_tools: Vec<String>,  // narrower allowlist the harness advertises
    pub env: HashMap<String, String>,
}
```

The fields are generic *enough* — `model_id` is a string, `env` is
free-form. Different harnesses pick which fields are meaningful.
Ollama probably ignores `plan_mode`. Claude CLI uses everything.

## Listener

```rust
#[async_trait]
pub trait Listener: Send + Sync {
    fn session_id(&self) -> &str;
    async fn on_petal(&self, kind: &str, payload: Value);
    async fn on_roost(&self, nest_id: &str, model: &str, tools: Vec<String>);
    async fn on_wilt(&self, finish_reason: &str, usage: Option<Value>, provider_meta: Value);
    async fn on_thorn(&self, wound: &str);
}
```

humd's binary registers a `Listener` on the roost when it accepts a
`chi:"prompt"`. The listener turns parsed petals into `chi:"chunk"`
tones on the thrum side. The mapping is:

| listener callback | thrum chi |
|---|---|
| `on_petal("text", …)` | `chi:"chunk"` (text part) |
| `on_petal("tool_use", …)` | `chi:"tool-call"` |
| `on_roost(...)` | `chi:"session-ready"` |
| `on_wilt(...)` | `chi:"finish"` |
| `on_thorn(...)` | `chi:"error"` |

## Encoding helpers

Three small helpers convert higher-level intents into the
stream-json frames Claude CLI expects on stdin. They live here
because every pipe-mode perch uses them, but the daemon can call
them directly:

```rust
nest::encode_prompt("hello")             // user message frame
nest::encode_tool_result(call_id, body)  // tool_result frame
nest::encode_cancel(request_id)          // control_cancel_request frame
```

Future non-Claude harnesses with different stdin formats will
provide their own encoders.

## What this crate is NOT

- **Not a thrum endpoint.** Roosts don't speak thrum directly —
  humd reads petals via `Listener` and writes the thrum side.
- **Not a router.** `Nest` picks the right roost for a sid; it
  doesn't decide which humd handles which conversation. Ensemble owns
  that.
- **Not a transcript store.** PipePerch hands events to the
  listener; persistence is the harness's problem (claude-cli's graft
  module writes JSONL).
- **Not a permissions enforcer.** The daemon's permit handler runs
  before a `chi:"tool-call"` ever lands here. nest just sees what
  the model produced.
- **Not opinionated about content.** A `Petal` is just `Value`. The
  daemon (or the drone) decides whether text in it looks suspicious.

## Layout

```
nest/
├── src/
│   ├── lib.rs   # SpawnSpec, Roost, Perch trait, Listener trait, encode_* helpers
│   ├── pool.rs  # Nest — the roost pool runtime
│   └── mock.rs  # MockPerch for sim tests
└── README.md
```

Concrete `Perch` impls live under [`nests/`](../nests):

- [`nests/claude-cli/`](../nests/claude-cli) — `claude -p` stream-json over pipe
- [`nests/claude-repl/`](../nests/claude-repl) — PTY-mode interactive fallback
- [`nests/common/`](../nests/common) — shared building blocks across nest impls

## See also

- [WIRE.md](../WIRE.md) — the thrum protocol the listener emits into.
- [`drone/`](../drone) — the sentinel that observes petals and
  scores channel health.
- [`humd/`](../humd) — the daemon that owns the `Nest` instance.
- [vocabulary](../VOCABULARY.md) — words this crate uses load-bearingly.

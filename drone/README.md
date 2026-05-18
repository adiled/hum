---
title: "drone"
description: "hum's sentinel — observes every tone, scores channel health, flags context loss, returns a one-word verdict the host steers on"
---

# drone

> _hum's sentinel — observes every tone, scores channel health, flags context loss, returns a one-word verdict the host steers on_

The drone watches. It does not act. Every tone hum sends or receives
runs past the drone first; every LLM event (token, tool-call,
permission-ask, finish) gets observed; every heartbeat tick gets
counted. The drone keeps a small ledger per `sigil` — the
content-addressable handle for one nest's conversation — and on
demand returns a single-word **verdict** describing what the host
should do next.

The host owns the timer, the retry plumbing, the kill-and-respawn,
the resync. The drone owns the *judgment*.

## What it watches

| signal | source | what it tracks |
|---|---|---|
| outgoing tones | host calls `drone.sent(tone)` | pending echoes for `prompt` / `cancel` / `release-permit` (tracked chis) |
| incoming tones | host calls `drone.heard(tone)` | echo arrivals (clear pending), wane sync, missed-beat reset, pulse-death cleanup |
| LLM events | nestler calls `drone.observed(sigil, event)` | inflight tools, pending permissions, tokens burned, accumulated response text |
| heartbeats | host invokes `drone.mark_missed_beat(sigil)` on silence-timer expiry | missed-beat counter |

## What it returns

A single call: `drone.assess(sigil) -> Assessment`. The verdict is
the one thing the dispatch loop branches on:

| verdict | meaning | host should... |
|---|---|---|
| **Ok** | nominal — keep going | nothing |
| **Retry** | echo timed out for a tracked tone | re-send the tone; bump `note_retry` |
| **Drift** | `local_wane != remote_wane` and at least one beat seen | trigger `chi:"wane-sync"` reconciliation |
| **Dead** | missed beats past threshold (default 3) | tear the channel down; expect the host to re-open |
| **Swallow** | suspicious response — context loss confirmed | "wither" — kill the model process, re-send the last prompt; user sees no flicker |

The `Assessment` also carries a `raw: RawAssessment` with the full
diagnostic — health, suspicion tier, counters, the reason string —
even when the verdict is `Ok`. Useful for tracing, dashboards, and
the rare case where the host wants to *also* react to non-unified
signal.

## Health, rhythm, and the heartbeat

Health is the drone's *mood*, derived from the ledger. The rhythm is
how often the host should emit a `chi:"drone"` beat:

| health | trigger | rhythm |
|---|---|---|
| **Serene** | quiet channel, nothing inflight | 30 s |
| **Alert** | tool active or tokens flowing | 5 s |
| **Tense** | pending permission, > 3 inflight tools, or pending echoes | 1 s |
| **Critical** | missed beats ≥ 3, or wane drift, or echo deadline blown | 0.5 s |

The faster the rhythm, the sooner peers learn things are bad. The
beat payload (`DroneBeat`) includes the sigil, current wane, health
tier, rhythm, pending-echo rids, and a load summary. The host
serializes it into a `chi:"drone"` tone via `Drone::beat_body(beat)`.

## Context-loss detection (the *swallow* path)

The drone itself **knows nothing about LLMs.** What "context loss"
looks like in text is plugged in via the [`Classifier`] trait. The
default [`NoopClassifier`] never flags anything — a bare drone is a
pure channel-health sentinel and never fires `Swallow` on its own.

Detection is two-tier when wired:

1. **Classifier** (`drone::Classifier` trait): inspects text, returns
   one of four `Suspicion` levels:
   - `None` — text looks fine
   - `Soft` — flagged for evaluator-driven adjudication
   - `Heavy` — strongly flagged; evaluator may still confirm
   - `Critical` — bypass the evaluator and swallow immediately

   The regex-driven implementation tuned for chat-LLM context loss
   ("I don't have any previous context", greeting reset, identity
   reset, formality shift) lives in
   [`hives/common`](../hives/common) as `RegexClassifier` — not in
   this crate. Other nests can ship their own classifiers without
   touching drone.

2. **LLM judge** (optional, via `Evaluator` trait): when the
   classifier flags `Soft` or `Heavy`, the drone can consult a
   pluggable evaluator. `Critical` always swallows; the evaluator
   is skipped on the hot path. `Soft`/`Heavy` only swallow if the
   evaluator's score crosses `swallow_threshold` (default 0.7).

Suspicion is **independent of channel health** — a perfectly serene
channel may be spewing a context-loss greeting. The drone notices.

## Why this exists

Long-running LLM sessions occasionally drop their context window
mid-conversation. The model, robbed of history, fabricates a polite
"how can I help" or apologizes for not seeing prior messages. To the
user this looks like a model crash; to the host it looks like a
normal turn. Without the drone, hum keeps feeding context to a model
that has forgotten everything.

The drone's job: catch this *before* the user sees it. The cup (a
small buffer over the first ~80 bytes of each turn — see the TS
implementation in [`cup.ts`](cup.ts)) is the early-flag mechanism.
The verdict (`Swallow`) is what tells the host to wither and respawn.

## Wiring it in

The drone is `Clone` and cheap to share — all state lives behind an
`Arc<Inner>`. The host hands the same `Drone` to its send path, its
receive path, and the nestler's per-turn loop:

```rust
use drone::{Drone, Observed, Verdict};
use std::sync::Arc;

// Pure channel-health sentinel — no LLM context-loss detection.
let drone = Drone::new();

// Or, with a regex classifier from hives/common:
// let drone = Drone::with_classifier(Arc::new(nest_common::RegexClassifier));

// Send path
drone.sent(&outgoing_tone);

// Receive path
drone.heard(&incoming_tone);

// Nestler observes LLM events
drone.observed(&sigil, Observed::ToolStart { name: Some("read".into()) });
drone.observed(&sigil, Observed::TextDelta { text: chunk_text });
drone.observed(&sigil, Observed::TurnEnd);

// Periodically: assess + react
match drone.assess(&sigil).unified {
    Verdict::Ok => { /* nothing */ }
    Verdict::Retry => resend_last_tone(&sigil),
    Verdict::Drift => start_wane_sync(&sigil),
    Verdict::Dead => tear_channel(&sigil),
    Verdict::Swallow => wither_and_respawn(&sigil),
}

// On the rhythm tick: emit a beat
let beat = drone.beat(&sigil);
emit_tone("drone", Drone::beat_body(&beat));
```

For LLM judge plug-in (combined with a classifier):

```rust
struct LLMJudge;
impl drone::Evaluator for LLMJudge {
    fn evaluate(&self, text: &str, state: &drone::DroneState) -> f32 {
        // call an LLM, return probability of real context loss
        0.0
    }
}

let drone = Drone::with_classifier_and_evaluator(
    Arc::new(nest_common::RegexClassifier),
    Arc::new(LLMJudge),
    0.7,
);
```

## What this crate is NOT

- **Not a router.** The drone reads tones; it doesn't decide where
  they go. The host's dispatch loop owns routing.
- **Not a retry queue.** Pending echoes are tracked, but the host
  does the re-send. The drone just says "retry now" via verdict.
- **Not a respawner.** `Swallow` is a *recommendation* — the host
  owns the nest process and decides whether to kill it.
- **Not a metrics sink.** The `RawAssessment` is per-sigil
  diagnostic; for cross-sigil rollups, use [`drift`](../drift) or
  [`penny`](../penny).
- **Not opt-out at the crate level.** If you don't want the drone,
  don't construct one. (The TS side has a `stubDrone()` no-op for the
  off-switch in plugin config; the Rust side is silent enough that
  most hosts can keep it on with negligible cost.)

## Layout

```
drone/
├── src/
│   ├── lib.rs       # Drone, DroneState, Verdict, Assessment, observe API
│   └── classify.rs  # Suspicion heuristics (regex tiers)
├── classify.ts      # original TS heuristics (kept in lockstep)
├── cup.ts           # early-text buffer + wither trigger
├── drone.ts         # TS Drone runtime
├── index.ts         # TS exports
├── llm.ts           # TS LLM judge implementation (Claude-based)
├── prompts.ts       # TS judge prompts
└── README.md
```

The Rust crate is what `humd` links. The TS files are the original
implementation kept alongside while the OC plugin still ships TS;
algorithms match byte-for-byte.

## See also

- [WIRE.md](../WIRE.md) — `chi:"drone"` and `chi:"echo"` on
  the wire.
- [`ensemble/`](../ensemble) — drone state is per-sigil, not
  per-humd; ensemble routing happens above the drone.
- [`drift`](../drift) — drift rings: p50/p95 timing across humds,
  consuming `chi:"perf-mark"` tones.
- [`penny`](../penny) — lifetime counters (tokens, tool calls)
  across all sigils.
- [vocabulary](../VOCABULARY.md) — words this crate uses load-bearingly.

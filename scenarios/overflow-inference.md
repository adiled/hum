---
title: "overflow-inference"
description: "the asked-of humd is full; the work flows to the one with capacity"
---

# overflow-inference

> _the asked-of humd is full; the work flows to the one with capacity_

See `sim/tests/overflow_inference.rs` for the executable form.

## The setup

Trust tier **T3/T4** — federated or open mesh. Two humds with
asymmetric capacity:

- **humd-A** — gateway. Has a nestler attached (an `openai-server`
  bee fronting a public HTTPS endpoint, convention-stateful,
  medium richness). It announces `nests: []` for the requested model
  or `can_relay: true` with no local capacity. It is the door, not
  the kitchen.
- **humd-B** — worker. Hosts a `claude-cli` nest and advertises
  `nests: ["claude-cli"]`, `hosts: [...]` in its `PeerCapabilities`,
  with available inference slots.

Both humds belong to the same ensemble. Discovery has already
populated each side's peer registry with the other's caps. No
nestler is attached to humd-B; its role is purely to host hums on
behalf of routed prompts.

## The happy path

1. A client hits humd-A's `openai-server` nestler with a prompt for a
   model only humd-B can serve. The nestler emits `chi:"prompt"` into
   humd-A's daemon.
2. humd-A consults the ensemble: it has no local nest for this
   model and no spare slot regardless; humd-B advertises both.
   humd-A picks humd-B (capacity-aware scoring: free slots, RTT,
   advertised model coverage) and emits an `overflow.route` decision
   trace.
3. humd-A mints a fresh sigil for the routed hum and forwards the
   prompt as `chi:"prompt"` with `to: <humd-B HumdId>` and a
   bookkeeping field `origin: <humd-A HumdId>` so humd-B knows where
   to stream petals back.
4. humd-B accepts, spawns the brood on its local `claude-cli` nest,
   begins blooming. Every outbound petal is routed to humd-A as well
   as kept locally; humd-A forwards them onto the originating
   nestler's stream so the HTTPS client sees real-time chunks.
5. `chi:"finish"` lands on humd-B; it forwards to humd-A; humd-A
   closes the SSE/HTTP response with matching `usage`.
6. After close, humd-A still holds the full transcript replicated
   from humd-B — the gateway has a local copy for audit/retry, not
   just for live forwarding.

## The failure modes

- **Capacity lie.** humd-B advertised capacity but is in fact full.
  humd-A's route attempt must surface `chi:"error"` with
  `qualifier:"overflow.no-capacity"` and either retry against another
  peer or fail the client cleanly — never hang.
- **Drop mid-stream.** humd-B's link to humd-A drops between
  `chi:"chunk"` 5 and 6. The test must catch this as a routed-bloom
  error surfaced to humd-A's nestler, not a silent truncation. Bonus:
  on heal, wane allows humd-A to fetch the missing slice without
  re-prompting.
- **Replication gap.** humd-A receives the live chunks but the
  post-close audit copy is incomplete (missing tool-calls, missing
  drone, missing perf-marks). Test asserts the replicated transcript
  on humd-A is byte-equivalent to humd-B's local hum log for the
  sigil.
- **Misroute.** humd-A picks a peer that has the nest kind but not
  the specific model. The brood emits `chi:"error"` qualified
  `overflow.model-unavailable`; humd-A must propagate to the client
  with the same qualifier, not a generic 500.
- **No eligible peer.** No humd in the ensemble advertises the model.
  humd-A must fail synchronously with `overflow.no-route` before any
  network attempt, not after a timeout.

## The success criteria

- humd-A emits exactly one trace `nest.overflow.routed` naming the
  chosen `HumdId` before the first chunk is requested upstream.
- humd-B's local tap receives `chi:"prompt"` with `origin =
  humd-A.id` within `RTT + 50ms` of step 1.
- The HTTPS client connected to humd-A receives the first SSE chunk
  within `RTT_AB + RTT_AB + first-token-latency + 100ms` (one RTT to
  humd-B, one back for the first petal).
- The client's terminal SSE event carries `usage.output_tokens > 0`
  and matches the `usage` on humd-B's local `chi:"finish"` exactly.
- After close, humd-A's hum store for the routed sigil contains the
  full ordered petal log; comparison against humd-B's store yields
  zero diff. `WaneTracker::is_behind` on humd-A reads false against
  humd-B's tip.
- For each failure mode above, the corresponding error qualifier
  reaches the client within `RTT + 200ms` of detection.

## What this scenario validates

- **Capacity-aware routing.** `PeerCapabilities.hosts` /
  `caps.nests` plus advertised slot count drive a real selection
  decision, not just a lookup.
- **Transcript replication back to the requester.** The originating
  humd ends up with the full hum on disk, not just the live stream.
  This is what makes the gateway useful for retry, audit, and
  later attach-from-elsewhere.
- **Cross-humd `origin` tracking.** Petals flow back along an
  explicit return path, not through ambient broadcast.
- **Graceful failure on every degenerate input.** Lying peer,
  vanishing peer, model-shaped mismatch, empty mesh — each has a
  distinct qualifier surfaced to the client.
- **Routing surface under load.** Same `Ensemble::route` primitive
  as the other scenarios, here with the added pressure of choosing
  *which* peer when more than one could serve.

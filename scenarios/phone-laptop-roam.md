---
title: "phone-laptop-roam"
description: "one human, two devices, one hum that follows the body"
---

# phone-laptop-roam

> _one human, two devices, one hum that follows the body_

See `sim/tests/phone_laptop_roam.rs` for the executable form.

## The setup

Trust tier **T1** — own-devices. One operator with two humds:

- **humd-L** (laptop) — has the brood. Hosts a `claude-cli` nest, holds
  the live hum.
- **humd-P** (phone) — quiet at first. No hum hosted locally; runs only
  the daemon and a `vercel-ai` nestler embedded in a chat app.

Both humds share the same operator pubkey (T1 fingerprint), so their
`HumdId`s are siblings on the same identity chain. The in-memory
`Transport` wires them as peers from the start, but humd-P attaches no
nestler until the operator picks up the phone.

A single nestler on the laptop drives the hum during the first leg:
`opencode` plugin, rich, stateful. It speaks every chi.

## The happy path

1. humd-L mints a hum, sigil resolved, `chi:"hello"` exchanged with the
   OC nestler. Operator types the prompt. Bloom begins; petals flow.
2. Mid-bloom, operator walks away from the laptop. The nestler on
   humd-L stays attached; the brood keeps the perch warm; wane keeps
   ticking against the local sigil.
3. Phone wakes. humd-P's chat app opens; its `vercel-ai` nestler asks
   the local humd `peer-add` style: "attach me to hum `<sigil>`."
   humd-P doesn't host the hum, so it routes the attach request to
   humd-L over the ensemble link, naming the target by `HumdId`.
4. humd-L recognises the sibling identity, grants attach, and begins
   replicating its bloom: every fresh `chi:"chunk"` is also routed to
   humd-P, which fans it out to the phone's nestler.
5. The operator now sees the same petals on the phone, in order, with
   no replay of the petals they already saw on the laptop (cursor
   carried by `wane`).
6. Operator types a follow-up on the phone. The nestler emits
   `chi:"prompt"` into humd-P, which routes it to humd-L (the hum's
   home). humd-L feeds the brood; new petals flow back to both humds.
7. Operator closes the laptop. humd-L's local OC nestler detaches; the
   hum keeps living, now driven solely from humd-P's tap.

## The failure modes

- **Identity mismatch.** humd-P's `HumdId` is forged or wrong; humd-L
  must refuse attach with `RouteError`-style rejection and never leak a
  petal. Test asserts zero tones reach humd-P after the bad handshake.
- **Late attach loses petals.** Without wane the phone would either
  miss the chunks emitted between walk-away and attach, or get a flood
  of duplicates. Test must catch both: attach replays exactly the
  petals humd-P has not yet seen, and replays them in sigil-order.
- **Divergent wane.** If humd-L advances wane locally while humd-P's
  attach is in flight, the catch-up batch must close the gap; humd-P's
  `WaneTracker::is_behind` must read `false` once attach settles.
- **Stuck home.** If humd-L drops mid-bloom (process death, not
  network), humd-P's tap must surface `chi:"error"` with a peer-loss
  qualifier — not silence. Drone must stay quiet until the loss is
  surfaced.

## The success criteria

- After step 4, humd-P's nestler tap receives `chi:"hello"` referencing
  the same `sigil` humd-L holds, within `RTT + 50ms`.
- humd-P observes the full ordered sequence of `chi:"chunk"` petals
  from prompt-start to current-tip; the concatenated text equals
  humd-L's. No petal appears twice; no petal is missing.
- After step 6, humd-L's tap receives `chi:"prompt"` originated by
  humd-P within `RTT + 50ms`, with `from` set to humd-P's `HumdId`.
- The final `chi:"finish"` arrives on both taps with
  `usage.output_tokens > 0` and matching token counts.
- `ensemble.peers()` on humd-L lists humd-P throughout; on humd-P,
  lists humd-L. Neither registry shows stale entries after step 7.

## What this scenario validates

- **`hum_id` is location-independent.** A hum is named by its sigil,
  not by the humd that birthed it. Routing finds the hum wherever it
  lives.
- **peer-add wires connections.** The ensemble's `add_peer` +
  `Transport::connect` path is the same regardless of which humd
  initiates; T1 is identity-shortcut, not a different code path.
- **State propagates via wane.** Mid-stream attach replays exactly the
  missing slice. No bespoke catch-up protocol — the wane cursor does
  the work.
- **Routing surface.** `Ensemble::route` carries `chi:"prompt"` from
  humd-P to humd-L, and `chi:"chunk"` from humd-L to humd-P, both
  addressed by `to: <HumdId>`.

---
title: "co-pilot"
description: "two operators, one hum — one drives, one watches over the shoulder"
---

# co-pilot

> _two operators, one hum — one drives, one watches over the shoulder_

See `sim/tests/co_pilot.rs` for the executable form.

## The setup

Trust tier **T2** — known-circle. Two operators on two humds, paired
by manual key exchange:

- **humd-A** (driver) — hosts the hum. An `opencode` nestler attaches
  in normal stateful-rich mode, full keyboard, full bloom.
- **humd-B** (observer) — attaches a nestler with the **`hearOnly`**
  propensity. It is allowed to receive every chi but cannot emit
  `chi:"prompt"`, `chi:"tool-result"`, `chi:"permission-response"`, or
  `chi:"cancel"`. The brood enforces this at the humd boundary, not the
  nestler.

The pair completes a T2 handshake: both sides verify the other's
pubkey against a circle-of-trust list, agree on `proto_version`, and
swap `PeerCapabilities`. humd-B announces no hosted hums and no relay
willingness — it is here to listen.

## The happy path

1. Operator-A starts a hum on humd-A. Sigil minted. Bloom open.
2. humd-A registers humd-B as a peer with capability
   `nestler.role=hearOnly` and begins teeing every outbound petal to
   the ensemble link addressed to humd-B's `HumdId`.
3. Every `chi:"chunk"`, `chi:"tool-call"`, `chi:"tool-result"`,
   `chi:"permission-ask"`, `chi:"finish"`, `chi:"breath"`, even
   `chi:"perf-mark"` and `chi:"drone"`, that humd-A would emit to its
   own nestler is fan-out'd to humd-B over the wire. humd-B forwards
   them to its observer nestler in the same order they were emitted
   on humd-A.
4. Operator-A types a prompt. Petals flow on both humds, sigil-ordered.
5. Operator-B watches their screen update in real time. They try to
   type — their nestler may accept keys locally, but the moment it
   emits `chi:"prompt"` toward humd-A, the brood rejects it with
   `chi:"error"` qualified `hearOnly.denied`, and the prompt never
   reaches the nest.
6. A `chi:"permission-ask"` lands on both nestlers. Only Operator-A's
   reply is honoured; if Operator-B's nestler emits a response, humd-A
   drops it (logged via trace `permission.hold.denied.hearOnly`).
7. Hum finishes. Both nestlers receive the same terminal `chi:"finish"`
   with identical `usage` block.

## The failure modes

- **Fan-out skew.** humd-B receives petals in a different order, or
  with a different `rid` namespace, than humd-A's own tap. The test
  must compare the two tap transcripts and fail on any reordering.
- **hearOnly bypass.** humd-B's nestler successfully drives the hum
  (prompt accepted, permission-response honoured, cancel taken). The
  test must verify each forbidden chi is dropped at humd-A's boundary
  and that an `error.hearOnly.denied` is observable on humd-B.
- **Silent drop on the tee.** humd-A's own nestler keeps seeing
  petals, but humd-B sees nothing — the tee path failed without any
  observable error. Test asserts non-empty humd-B transcript and a
  surfaced `chi:"error"` if the tee ever breaks mid-bloom.
- **Late join double-replay.** humd-B attaches after the hum has
  already produced N chunks. It must receive those N chunks once,
  then live tail — not N more copies as duplicates, and not a fresh
  stream starting from chunk N+1 with no replay.

## The success criteria

- humd-B's tap receives `chi:"hello"` for the sigil within `RTT + 50ms`
  of humd-A's tap receiving its own `hello`.
- For every petal emitted to humd-A's nestler, humd-B's nestler
  receives an equivalent petal (same `chi`, same `rid`, same `sigil`,
  same payload) within `RTT + 50ms`. Transcripts compared after the
  hum finishes are byte-identical modulo `from`/`to` framing.
- humd-B's tap receives the terminal `chi:"finish"` with
  `usage.output_tokens > 0` and `usage.output_tokens` equal to
  humd-A's tap's value for the same sigil.
- For every forbidden chi emitted by humd-B during the run, humd-B's
  tap also receives a `chi:"error"` with `qualifier:"hearOnly.denied"`
  citing the rejected `rid`. The hum's transcript on humd-A is
  unchanged by these attempts.
- After the hum closes, both `WaneTracker`s report the same tip for
  the sigil; `is_behind` is false on both sides.

## What this scenario validates

- **Tee / fan-out across humds.** One hum, two taps, lockstep order.
  Same primitive that overflow and partition-heal will lean on later,
  here exercised in its clean form.
- **`hearOnly` semantics.** The propensity is enforced at the brood
  boundary on the *hosting* humd, not on trust at the nestler. An
  observer cannot drive even if compromised.
- **Capability-shaped admission.** `PeerCapabilities` carries enough
  to gate writes per peer; the daemon honours it without per-call
  policy reads.
- **Replication surface.** Same routing primitive as `phone-laptop-roam`
  but with a permanently-attached second tap rather than a roaming
  one — establishes that tee is steady-state, not just a catch-up tool.

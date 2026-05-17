# partition-and-heal

> _the wire breaks; both sides keep humming; the wire mends; the wanes reconcile_

See `sim/tests/partition_and_heal.rs` for the executable form.

## The setup

Trust tier — irrelevant; this scenario is about reconciliation
mechanics, not trust. Use **T2** for concreteness. Two humds, both
hosting nestlers, both attached to the **same hum**:

- **humd-A** — has a nestler driving. Owns one half of the ongoing
  hum's local wane.
- **humd-B** — has a second nestler also attached, kept in lockstep
  via the tee from `co-pilot`. Owns the other half.

Both share the sigil. `WaneTracker` on each side advances as petals
land locally. The in-memory `InMemoryEndpoint` between them is wrapped
in a sim middleware that can be commanded to **partition** (drop all
tones in both directions) and **heal** (resume delivery, optionally
with a backlog flush from a buffered queue).

A petal source — a real or simulated brood — is colocated with
humd-A and continues producing petals during the partition.

## The happy path

1. Hum opens. Both nestlers see `chi:"hello"`, then a clean prefix of
   `chi:"chunk"` petals. Wanes on both sides advance in lockstep up
   to tick `N`.
2. Sim partitions the link between humd-A and humd-B. From this
   moment, no tones cross. Both daemons trace
   `ensemble.peer.unreachable` for the other id; **drone stays quiet**
   — no panic, no spammy retry storm, no surfaced error to the
   nestlers beyond a single `chi:"breath"` qualified
   `peer.partition`.
3. humd-A's brood continues producing petals. humd-A's local nestler
   sees `N+1, N+2, …, N+k`. humd-A's wane advances to `N+k`.
4. humd-B, being a tee-only target for this hum, receives nothing
   during partition. Its wane stays pinned at `N`. Its nestler sees
   no fake petals, no replay loop, no synthetic activity. It simply
   waits.
5. Sim heals the link. Both daemons trace `ensemble.peer.reachable`.
6. Each side issues `wane.compare` against the other (the
   reconciliation handshake). humd-A discovers humd-B is behind by
   `k` petals for this sigil; humd-B discovers it is behind. humd-A
   replays petals `N+1 … N+k` in order, exactly once.
7. humd-B's nestler observes the `k` missing petals appear in order
   after a silent gap. Its wane catches up to `N+k`. From `N+k+1`
   onward, both humds resume lockstep tee.

## The failure modes

- **Duplicate petals.** Reconciliation replays a petal humd-B already
  has, perhaps because the partition happened to drop only one
  direction. Test asserts humd-B's transcript shows each `rid`
  exactly once across the full run.
- **Lost petals.** Reconciliation misses a petal — humd-B's
  transcript is missing some `N+i`. Test asserts strict
  monotonicity: every tick from 1 to final `M` is present on both
  sides.
- **Divergent wane after heal.** Either `WaneTracker::is_behind`
  remains true after reconciliation settles, or the two tips
  disagree. Test asserts: 30 seconds (sim time) after heal, both
  tips equal and `is_behind == false` on both sides.
- **Drone noise during partition.** humd-A or humd-B's drone fires
  during the partition window with false-positive errors or
  speculative re-prompts. Test sets a drone-quiet assertion across
  the partition window — no `chi:"drone"` with severity >= warning
  while the peer is known-unreachable for the same hum.
- **Split-brain prompts.** humd-B's nestler manages to emit a
  `chi:"prompt"` during partition (it shouldn't, the hum is hosted
  on humd-A). The local humd-B daemon must reject with
  `chi:"error"` qualified `peer.partition.write-denied`. After
  heal, no buffered prompts must replay.
- **Heal storm.** Reconciliation floods the wire with every petal
  ever produced instead of the diff slice. Test asserts the number
  of tones flowing during reconciliation is bounded by the wane
  delta, not by total bloom size.

## The success criteria

- During the partition window, humd-B's tap receives zero
  `chi:"chunk"` tones. humd-A's tap receives the live stream
  uninterrupted.
- During the partition window, both humds trace
  `ensemble.peer.unreachable` exactly once per direction; neither
  emits a `chi:"drone"` of severity warning-or-higher attributed to
  the unreachable peer.
- After heal, humd-B's tap receives exactly `k = wane_A - wane_B`
  petals before any new petals, in strict sigil-order, within
  `RTT + 100ms` of heal detection.
- After reconciliation, both `WaneTracker`s report the same tip for
  the sigil and both `is_behind` queries return false.
- For every `rid` produced by the brood during the entire run, both
  humds' final transcripts contain it exactly once. Set-equality
  with multiplicity asserted programmatically.
- The terminal `chi:"finish"` arrives on both taps with
  `usage.output_tokens > 0` and identical counts.

## What this scenario validates

- **Wane convergence.** The lazy, Matrix-shaped reconciliation
  protocol in `thrum_core::WaneTracker` survives an arbitrary-length
  partition with no replay duplicates and no missing petals.
- **No duplicate / no lost petals.** The catch-up slice is precisely
  the diff, computed from wane comparison, not from time windows or
  full re-sync.
- **Drone-quiet during partition.** A known peer being unreachable
  is a single observable event, not a recurring alarm. The drone
  policy distinguishes "we know it's gone" from "something is
  wrong."
- **Partition tolerance surface.** `Ensemble` keeps the peer in its
  registry across the outage, marks it unreachable, and resumes
  routing on heal — no peer flap, no churn in `peers()`.
- **Locality of authority.** The hum is hosted on humd-A; humd-B
  cannot accept writes for it during the partition, and the system
  refuses to fabricate a split-brain history.

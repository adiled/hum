---
title: "find-the-worker"
description: "the prompt names a model; the mesh decides which humd runs it"
---

# find-the-worker

> _the prompt names a model; the mesh decides which humd runs it_

See `sim/tests/remote_discovery.rs` for the executable form.

## The setup

Trust tier **T3/T4** — federated or open mesh. Two humds, asymmetric
by kind rather than by capacity:

- **humd-L** — the laptop. Has a nestler attached and **no worker at
  all**. It cannot answer a single model on its own.
- **humd-S** — the server. Hosts a `claude-cli` worker nest advertising
  `models: ["claude-opus-4-7"]`. No nestler is attached; its role is
  purely to run work humd-L asks for.

humd-S has already advertised its worker on `hum/hives/announce` when
that bee handshook with its own humd. humd-L subscribed at startup and
holds the manifest under humd-S's Hid.

## The happy path

1. A nestler on humd-L emits `chi:"prompt"` with a `modelId` and
   **no `to`**. It is not naming a machine. It cannot; it has never
   heard of humd-S.
2. humd-L finds no local worker advertising that model. It consults the
   manifests it heard over gossip, keeps only humds still in
   `ens.peers()`, and picks one advertising a worker for that model.
   Emits a `prompt.forward.remote` trace naming the chosen peer.
3. humd-L sets `to: <humd-S Hid>` and `from: <humd-L Hid>` and routes
   the tone. It does not run the session and does not claim the sigil.
4. humd-S sees the prompt with `client_id == "ensemble"`, reads
   `from` as its `origin`, and runs its **own** local worker selection —
   the same by-model lookup it would run for a local nestler.
5. The worker emits `chi:"chunk"` then `chi:"finish"`. humd-S routes
   each one to `sid_origins[sid]` (humd-L) *and* broadcasts locally.
6. humd-L receives each tone with `client_id == "ensemble"`, matches on
   `sid`, and broadcasts onto its nestler's stream. The nestler sees
   `finish` and the turn closes.

## The failure modes

- **Nobody has the model.** No local bee, no advertised remote. The
  nestler gets `chi:"error"` naming the model. If the caller was itself
  remote, that error is routed back to its origin rather than dying in
  the local session and leaving the origin to time out.
- **The manifest outlived its peer.** Gossip carries no timestamp and a
  bee advertises only when it handshakes with its *own* humd, so a
  manifest can outlive the peer that made it reachable. Selection is
  gated on `ens.peers()`, and `PeerRemove` drops that humd's manifests,
  so a dead humd is never chosen.
- **The peer dies between selection and route.** The forward fails and
  **falls through** to the error reply. It must not return silently —
  that would hang the caller until its own timeout.
- **The peer reconnects.** A reconnect does not re-trigger the bee's
  handshake with its own humd, so nothing re-advertises it. `PeerAdd`
  re-advertises what we already know, closing the window where a
  reachable humd stays invisible.
- **Several humds could serve the model.** Selection is deterministic
  (sorted by Hid) rather than capacity-aware. Capacity-aware selection
  is `pick_overflow_peer`'s job; two mechanisms, deliberately not
  unified here.

## The success criteria

- The nestler's tone carries no `to`, and humd-S still runs the turn.
- Exactly one `prompt.forward.remote` trace, naming the humd chosen.
- `chi:"finish"` reaches the originating nestler on humd-L.
- Every reply carries `to: <humd-L Hid>` — the return path is explicit,
  not ambient broadcast.
- Without the discovery feed the same tone yields
  `no worker bee advertises model 'claude-opus-4-7'` and no `finish`.

## What this scenario validates

- **Discovery feeding routing.** `hive_advertise` was already called on
  every bee handshake and `hive_discover` existed with no production
  caller. This is the seam joining them to `Ensemble::route`.
- **The remote table is humd-keyed, not bee-keyed.** A bee has no
  ensemble presence of its own; only its humd is dialable, so the Hid
  returned by discovery is the routing address and the bee is the value.
- **The return path was already load-bearing.** `from` parsing,
  `sid_origins`, and reply routing all predate this. Only the first hop
  was missing — which is why the fix is one branch.
- **Staleness discipline without a lease.** Manifests carry no clock, so
  the peer set is the honest liveness signal and no TTL can be honest
  here.
- **A failed forward still answers.** Degrade to a real error rather
  than a hang.

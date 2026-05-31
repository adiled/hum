---
title: "thehum"
description: "per-humd authored chi log — a signed append-only NDJSON ring of every event the humd observes or authors"
---

# thehum

> _per-humd authored chi log — a signed append-only NDJSON ring of
> every event the humd observes or authors_

`thehum` is the sustained signal underneath a humd. Every chi that
crosses the daemon, whether the humd authored it or heard it from a
peer, lands here first as a signed, sequenced, hash-chained line.
Nothing else in hum is authoritative. `bees.json`, sid → bee routes,
session state, nest occupancy, ensemble's peer table — every one of
them is a projection rebuilt by replaying the log.

## The architectural law

> _identities and config are local; everything else is the log._

The humd's signing key lives on disk. Its `hum.json` lives on disk.
That is the entire local-only surface. State that anyone else might
ever care about — what happened in a bloom, which bee answered, what
the wane was, which tools fired — is a chi event in the log and
nowhere else. Crash, reboot, swap machines: open the log, `replay()`,
state returns.

This makes the daemon a deterministic function of its log. It also
makes a humd's history portable: copy the directory, copy the key,
and the same humd lives on a new host.

## Retention modes

A humd chooses one of three modes in `hum.json`. The mode controls
what disk holds; the in-memory tail is always live.

| mode | what it keeps | when to pick it |
|---|---|---|
| **archive** | every daily file, forever | research humds, on-chain anchors, audit operators |
| **rolling** | daily files within the last N days (default 30) plus all snapshots | the default — most desktop and server humds |
| **light** | only the latest daily file plus snapshots | phones, embedded boards, kiosk frontends |

Older daily files are pruned by `enforce_retention()`, which is
called from a humd timer. Snapshots survive every mode; replay can
always seed from the most recent snapshot instead of from genesis.

## Three flavors of write

Every line in the log was written by one of these three paths.
They share the same on-disk shape; only the provenance differs.

- **Authored.** This humd appends its own chi as the event happens.
  `event.author` is this humd's hid; `sig` is over the canonical
  bytes with the humd's key.
- **Replicated.** A peer's signed events arrive via `chi:"backfill"`.
  Each event is verified locally — signature, hash chain, monotone
  seq under that author — and then stored alongside the humd's own.
- **Snapshot.** Periodically, the humd hashes its current state-view
  Merkle-style and emits a `chi:"snapshot"` event into the log
  itself. Snapshots are ordinary events; replay treats them as a
  resume point rather than a primitive.

## Event shape

One JSON object per line, NDJSON throughout.

```json
{
  "chi": "prompt",
  "sid": "<HumId>",
  "rid": "<HumId>",
  "body": { },
  "author": "humd_<hex>",
  "seq": 102945,
  "ts_ms": 1719793480123,
  "prev_hash": "<32-byte hex>",
  "sig": "<ed25519 hex>"
}
```

`seq` is strictly monotonic per author. A gap is a missing event,
which is what triggers the next `chi:"backfill"`. `prev_hash` chains
each line to the canonical bytes of the previous one under the same
author — any tampered or reordered line fails verification.

## Config

`hum.json` — the `thehum` section:

```json
{
  "mode": "rolling",
  "days": 30,
  "snapshot_every_events": 1000,
  "snapshot_every_seconds": 600,
  "encrypt_at_rest": false,
  "fsync_per_event": true
}
```

| key | meaning |
|---|---|
| `mode` | `archive` \| `rolling` \| `light` |
| `days` | rolling-mode horizon, in days |
| `snapshot_every_events` | emit a snapshot after this many appends |
| `snapshot_every_seconds` | emit a snapshot at least this often |
| `encrypt_at_rest` | XChaCha20-Poly1305 with a key derived from the humd's signing key |
| `fsync_per_event` | flush every append; cost is durability vs. throughput |

## API surface

```rust
TheHum::open(dir, signing_key, Config) -> Result<Self>

thehum.append(chi, sid, rid, body)        -> Future<Seq>
thehum.tail()                             -> broadcast::Receiver<Event>
thehum.range(author, from_seq)            -> Vec<Event>
thehum.replay(handler)                    -> ()
thehum.snapshot(state_leaves)             -> StateRoot
thehum.enforce_retention()                -> RetentionReport
thehum.anchor(&backend, &root, height)    -> AnchorReceipt
```

`append` returns the seq assigned to the new event. `tail` is the
live broadcast every projection subscribes to. `range` answers a
`chi:"backfill"` from a peer. `replay` walks every stored event in
seq order, handing each one to a pure handler.

## Determinism rule

Materialized state derives purely from event content.

```text
handler(prior_state, event) -> next_state
```

No `now()`, no `rand`, no env reads, no filesystem peeks. The
author's `ts_ms` is the only time source — set once at append, never
re-derived. Any humd replaying the same range of events arrives at
the same state. Two humds disagreeing on derived state means they
hold different events, not different code.

## On-chain anchor

When a humd is on a network that adopts an anchor contract, it can
periodically publish its state root for discoverability, trust-free
timestamping, and dispute settlement.

```solidity
function anchor(
    bytes32 hid,
    bytes32 root,
    uint256 height,
    bytes   calldata sig
) external;
```

Anchoring is a `trait AnchorBackend`; an `EvmAnchor` scaffold prepares
the calldata so a wallet (or a delegated signer) can submit the
transaction. The log doesn't sign chain transactions itself — it
hands the calldata up. The receipt comes back as a `chi:"anchor"`
event, written into the log like any other.

## File layout

```
$XDG_STATE_HOME/hum/thehum/
  2026-05-31.ndjson         # today's authored + replicated events
  2026-05-30.ndjson         # yesterday's
  …
  seq.bin                   # last-persisted seq counter
  snapshots/
    <height>.bin            # one snapshot per height
    root.txt                # hex of the most recent state root
```

Daily files are the unit of retention; snapshots are the unit of
fast restart. Both are addressable by name alone, so a humd can
ship a single day, a single snapshot, or its whole history with no
extra index files.

## See also

- [`ensemble/`](../ensemble) — carries `chi:"backfill"` between humds.
- [`humd/`](../humd) — owns the `TheHum` instance and feeds it.
- [WIRE.md](../WIRE.md) — the chi values that ride the log.
- [`ids/`](../ids) — `HumId`, the content-addressed handle used for
  authors and sids.

# scenarios

> _prose specs of ensemble narratives — one story per file, each paired 1:1 with a sim test_

The ensemble is the mesh of humds. This directory is its libretto. Each
file names one situation a real user or pair of users might find
themselves in — roaming across devices, watching a peer drive, handing
work to another org, healing a partition — and tells the story of what
the wire is supposed to do under it.

Prose lives here so the *shape* of the narrative can be argued without
the noise of test fixtures. The matching test under `sim/` asserts the
same story in Rust, against the in-memory ensemble transport
(`ensemble::InMemoryEndpoint`) wired by [`/root/clwnd/sim/`](../sim/).

Pairing is strict — one MD, one test:

| scenario | test |
|---|---|
| `phone-laptop-roam.md` | `sim/tests/phone_laptop_roam.rs` |
| `co-pilot.md` | `sim/tests/co_pilot.rs` |
| `federation-handoff.md` | `sim/tests/federation_handoff.rs` |
| `overflow-inference.md` | `sim/tests/overflow_inference.rs` |
| `partition-and-heal.md` | `sim/tests/partition_and_heal.rs` |

Each MD covers five sections in the same order: **setup**, **happy
path**, **failure modes**, **success criteria**, **what this validates**.
The first four describe the story; the fifth names the protocol surface
under test (routing, replication, wane convergence, federation,
capacity-aware overflow, partition tolerance). When the test drifts from
the prose, fix one to match the other — they are meant to read like the
same document in two registers.

For the daemon-native shape these scenarios exercise — `HumdId`,
`PeerCapabilities`, `Transport`, `Ensemble::route` — see
[`/root/clwnd/ensemble/`](../ensemble/). Wane tracking lives in
`thrum-core::WaneTracker`. Trust tiers (T1 own-devices through T4 open
p2p) appear here as setup parameters; the daemon code is identical
across tiers, only the `Transport` impl swaps.

When you add a new narrative, copy the five-section skeleton from any
existing file, drop the test stub into `sim/tests/`, and add the row
above. The prose is the contract; the test is the witness.

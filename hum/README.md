---
title: "hum"
description: "The user-facing operator CLI. Status, logs, hive lifecycle, bee lifecycle, thehum inspection, mesh state."
---

# hum

> _the operator's hands on a running humd. Status, logs, hives, bees, the chi log, the mesh._

The [`humd`](../humd) binary runs the daemon. `hum` is the CLI the
operator types at the terminal to look at what humd is doing, change
its configured peers, install a new hive, start or stop a bee, audit
the persistent chi log, and (soon) talk to the daemon over an admin
RPC for live mesh state.

```bash
hum                          # one-line summary
hum status                   # daemon binary, identity, config, socket (paths only)
hum logs [-n LINES]          # tail recent daemon logs (journald or files)
hum doctor                   # one-shot full diagnostic for bug reports
hum ensemble                 # show identity, reach hints, configured peers
hum ensemble peer add <id> --hint <X> [--alias <name>]
hum ensemble peer rm  <id|alias>
hum hive --list              # the catalogue, and what is installed here
hum hive <ref> install       # teach this humd a hive (runs its hive-install)
hum hive <ref> uninstall     # forget it
hum bee --list               # bees entered here, and which have nestled
hum bee enter --hive <hive> [--as <label>]  # sign a bee in; prints its id
hum bee <id> exit|reenter
hum comb [hive]              # the comb: lifetime measures
hum thehum status|tail|range|verify|replay
hum recipes [name]           # list / run recipes
hum uninstall                # stop + remove binary (state preserved)
hum update [--force]         # check upstream release, self-update
```

## Design: on-disk artifacts first

By design every `hum` subcommand today reads what humd has written to
disk, not the running daemon. That has two consequences worth knowing.

First, every subcommand runs cleanly with humd stopped. `hum status`,
`hum ensemble`, `hum thehum verify` all work post-mortem and across
upgrades. State you can see when the daemon is up is the same state
you can see when it is down (modulo wall-clock).

Second, none of the live-only mesh state is reachable yet:
which peers actually completed signed hellos, what `learned_caps` the
ensemble drainer has on each peer, the current kad routing table,
in-flight blooms. That data lives in the running `Arc<Ensemble>` and
needs an admin RPC into the daemon. Today `hum ensemble` shows you
the inputs (identity + peers.json + reach hints from `RuntimeInfo`),
not the outputs. The same admin RPC will fill in `connected`, `caps`,
and `kad table` when it lands.

## Subcommands

### `status`
Daemon binary path, version, identity location, peers.json location,
hum.json location, thrum socket existence. All paths come from
[`hum-paths`](../hum-paths). Read-only.

### `logs`
Cross-platform tail. On Linux, `journalctl --user -u hum`. On macos
and others, tails `~/.local/share/hum/log/*.log`. Subset selection via
`-n`.

### `doctor`
One-shot full dump for bug reports: versions, config, env, the
`claude` binary status, every bee and service state, recent daemon
logs and nurse logs with warnings highlighted. Run it first; paste
the output into a github issue.

### `ensemble`
Inspect or edit on-disk peer-mesh state.

| invocation | effect |
|------------|--------|
| `hum ensemble` | print `me` (humd_id), `reach` (paste these into a peer's peers.json), `peers` (configured) |
| `hum ensemble peer add <humd_id> --hint <X> [--hint <Y>] [--alias <name>]` | append entry to peers.json (atomic), idempotent (replaces same humd_id) |
| `hum ensemble peer rm <humd_id|alias>` | drop matching entries |

Hints are wire-shaped strings: `tcp:host:port`, `iroh:<64-hex>`,
`iroh-ip:<sockaddr>`. The `reach` field of `hum ensemble` is exactly
what a remote peer pastes in.

### `hive`
Hives are the kinds this humd understands. A directory with an
`Orchfile` is a hive. `hum hive --list` shows the catalogue (`hives/`),
what is installed here, and how many bees each has entered. `hum hive
<ref> install` resolves the ref (bundled name, local path, or github
tree URL), records the hive at `~/.config/hum/hives.d/<hive>.orch`, and
runs its `hive-install` unit if it declares one, with `HUM_SRC`,
`HUM_HIVE`, and `hum` on its `PATH`, so the unit can enter bees and start
whatever it runs. A failed unit takes the record back. `hum hive <ref>
uninstall` forgets the hive; what the hive started is its own to stop.

### `bee`
Bees are the participants humd hears from. A bee is a key its hive
entered. `hum bee enter --hive <hive> --as <label>` signs one in under
the hive and prints its id: the same bee every time for one label, so a
process can enter itself at each start and a runtime can hold one bee per
requester. Whatever acts as the bee signs its hello with the key. `hum bee
<id> exit` signs it out, so humd refuses its next hello, and `reenter`
signs it back in. `hum bee --list` shows every entered bee and whether
it has nestled. hum starts no process.

### `thehum`
The signed append-only chi log on disk.

| verb | effect |
|------|--------|
| `status` | dir, file count, total seq, latest snapshot |
| `tail [-n N]` | most recent daily file (default 20 events) |
| `range --author <hid> --from <seq> [--to <seq>]` | filter by author + seq range |
| `verify` | check hash chain + signatures across the whole log |
| `replay` | count events by chi kind |

The chi log is the only authoritative record of what crossed the
daemon. Verifying it after a crash or after a migration is the first
move before trusting derived state.

### `comb`
The kernel's ledger of what crossed it, from `comb.json`: how many
tones of each chi, and nectar — tokens in, tokens out, cache reads —
per hive. `hum comb` shows the whole comb; `hum comb claude-cli` shows
one hive. The measure is written by the daemon, so it is observed
rather than self-reported.

### `recipes`
Recipes live under `recipes/`. `hum recipes` lists; `hum recipes <name>`
runs the recipe. Used for one-shot integrations (opencode setup, etc.)
that aren't long-running services.

### `uninstall`
Stops the service via the service manager and removes the `humd`
binary. State on disk (identity, peers.json, thehum log) is
deliberately preserved so re-installing later picks up the same
identity and the same peer relationships.

### `update`
Checks `github.com/adiled/hum/releases/latest`, compares to the local
version, and (if newer or `--force`) re-runs the canonical installer
which atomically bounces the service.

## Auto-completion

The CLI uses clap with derive macros, so `hum <Tab>` completion is
available via clap's standard completion machinery. Shell-side hookup
is on the operator (eventually a `hum completion <shell>` will emit
the script).

## See also

- [`humd`](../humd): the daemon `hum` operates on.
- [`hum-paths`](../hum-paths): everything path-shaped flows through here.
- [`ensemble`](../ensemble): the mesh `hum ensemble` reflects (today, via on-disk artifacts).
- [`thehum`](../thehum): the chi log `hum thehum` inspects.

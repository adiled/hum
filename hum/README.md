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
hum hive --list              # bundled + configured + running
hum hive <ref> install       # build + register + orchd up
hum bee --list               # bees and their state
hum bee <name> enter|exit|reenter
hum nest                     # orchd-managed bees (delegates to orchd status)
hum penny                    # lifetime counters
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
logs and worker logs with warnings highlighted. Run it first; paste
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
Hive kinds. A hive is the typology slot; a bee is the running
instance. `hum hive --list` shows the catalogue (installer present in
`hives/`), the configured slot (`hives.<kind>` in `hum.json`), and
running state. `hum hive <ref> install` resolves the ref (bundled
name, local path, or github tree URL), runs the auto-detected build
(cargo / pnpm / go / `./build`), copies the Orchfile under
`~/.config/hum/orch.d/<kind>.orch`, rewrites `hum.orch`, and runs
`orchd up <kind>`.

### `bee`
Bee instances. `hum bee --list` shows running bees by name and state.
`hum bee <name> enter|exit|reenter` flips lifecycle (`enter` starts a
stopped bee, `exit` stops while preserving state, `reenter` is a
graceful restart that keeps the same id).

### `nest`
Delegates to `orchd status` for the orchd-managed bees view.

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

### `penny`
Lifetime counters from `penny.json`: token swaps, tool executions,
session counts. The wallet, in metaphor.

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

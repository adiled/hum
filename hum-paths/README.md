---
title: "hum-paths"
description: "Single source of truth for every on-disk path hum reads or writes. Layout follows XDG Base Directory. Nothing else in the workspace hand-writes `~/.config/hum/...` or `$XDG_STATE_HOME/hum/...`."
---

# hum-paths

> _every filesystem path hum touches lives in this crate. Other crates ask, never assume._

If a path appears in another crate as a literal (`"~/.config/hum/..."`,
`PathBuf::from(env!("HOME")).join(...)`, hard-coded socket paths,
hard-coded state directories), that is drift. The convention is one
direction only: anywhere in the workspace that wants a path goes
through `hum_paths::*`. This crate is small and stable on purpose, so
the rest of the workspace can lean on it.

## Layout

hum keeps five XDG roots, and each holds only what
[`layout::LAYOUT`](src/layout.rs) declares. While humd runs, anything
else is moved to that root's `tmp/`.

| root | resolves to | holds |
|---|---|---|
| state | `$XDG_STATE_HOME/hum` | `humd.key`, `thrum.sock`, `runtime.json`, `routing.json`, `bees.json`, `revoked.json`, `bees/`, `bees.d/`, `thehum/`, `hives/<hive>/` |
| config | `$XDG_CONFIG_HOME/hum` | `hum.json`, `peers.json`, `hives.d/`, `hives/<hive>.json` |
| data | `$XDG_DATA_HOME/hum` | `src/` |
| cache | `$XDG_CACHE_HOME/hum` | `hives/` |
| runtime | `$XDG_RUNTIME_DIR/hum` | `comb.json`, `hum.sock.http` |

Every concrete path is one accessor down from these:

```rust
hum_paths::humd_key()        // state_dir/humd.key
hum_paths::hum_json()        // config_dir/hum.json
hum_paths::peers_json()      // config_dir/peers.json
hum_paths::thrum_sock()      // state_dir/thrum.sock
hum_paths::comb()            // runtime_dir/comb.json
hum_paths::thehum_dir()      // state_dir/thehum/
hum_paths::runtime_info()    // state_dir/runtime.json
hum_paths::bees_snapshot()   // state_dir/bees.json
hum_paths::bee_key(id)       // state_dir/bees/<id>.key
hum_paths::bee_entry(id)     // state_dir/bees.d/<id>.json
hum_paths::hive_installation(hive) // config_dir/hives.d/<hive>.orch
hum_paths::hive_state_dir(hive)    // state_dir/hives/<hive>/
hum_paths::hum_bin(kind)     // local_bin/<kind>
hum_paths::humd_bin()        // local_bin/humd
```

## `init()`

Call once at process startup, before any other accessor.

```rust
hum_paths::init();
```

`init()` reads `HOME` and sets any unset XDG env var to its
HOME-relative default. After it runs, every subsequent accessor in the
process resolves cleanly off the env without fallback logic. Without
`init()` first, accessors will still work in most cases (the XDG vars
are usually already set), but tests and embedders that need explicit
XDG control rely on `init()` having normalized them.

`init()` panics if `HOME` is unset. That is always a configuration
error, not something a graceful fallback should hide.

## `RuntimeInfo`

The one in-process write surface this crate owns is `RuntimeInfo`, a
small struct humd writes to `state_dir/runtime.json` when it
boots and removes when it shuts down. Other tools (`hum status`,
`hum ensemble`, dev scripts, third-party probes) read it to learn the
running daemon's pid, socket path, version, and the wire-shaped reach
hints (`ensemble_addrs`: `iroh:<node>`, `iroh-ip:<sockaddr>`, `tcp:...`)
a peer would paste into its `peers.json`.

```rust
let info = RuntimeInfo {
    socket: bound_socket.to_path_buf(),
    pid: process::id(),
    version: env!("CARGO_PKG_VERSION").into(),
    thrum_version: thrum_core::THRUM_VERSION.into(),
    bound_at_ms: now_ms(),
    ensemble_addrs: peer_reach,
};
info.write()?;                          // atomic: tmp + rename
```

`RuntimeInfo::read()` returns `Option<Self>`. Missing or unreadable
file resolves to `None`, never an error. The file is best-effort
discovery, not a contract; the daemon survives without it (and
removes it on clean shutdown).

## What this crate does not own

- **Config values.** `hum_paths::hum_json()` returns the path to
  `hum.json`. Loading it, validating against the schema, and parsing
  the values lives in [`config`](../config).
- **Network addresses.** A socket address is not a filesystem path.
  `humd.tcpListen = "0.0.0.0:14601"` lives in `config`, not here.
- **Repo-source paths.** [`codegen::paths`](../codegen) owns
  `thrum-core/src/chi.rs` and `thrum-clients/{ts,python,go}/...` because
  those are build-time, repo-local concerns, anchored to
  `CARGO_MANIFEST_DIR`. hum-paths is runtime/user-side, anchored to
  XDG.

The split is deliberate. hum-paths answers "where does humd find
runtime stuff on the user's host". codegen::paths answers "where in
the source tree does codegen read inputs and emit outputs". Different
concerns; no overlap.

## Adding a path

When a new artifact lands somewhere on disk:

1. Add a `pub fn foo() -> PathBuf` that returns the resolved path.
   Anchor on `state_dir()` / `config_dir()` / `data_dir()` /
   `runtime_dir()` as fits the lifecycle of the artifact.
2. Declare it in `layout::LAYOUT`, or the guard moves it to `tmp/` the
   moment it appears, and add it to `tests/layout.rs`.
3. Add a basename const if the file name is reused elsewhere (the
   `*_BASENAME` constants in `src/lib.rs` are the pattern).
4. Call it from the rest of the workspace, and write it with
   `write_atomic`. Never let a literal `~/.config/hum/...` or a
   `state_dir()`-style join land in another crate; `tools/paths.py`
   fails the build if one does.

## See also

- [`humd`](../humd): the daemon, the heaviest user of these paths.
- [`hum`](../hum): the operator CLI, surfaces them in `hum status` / `hum ensemble`.
- [`thehum`](../thehum): writes into `state_dir/thehum/`.
- [`config`](../config): loads from `config_dir/hum.json`, validated against `hum.schema.json`.

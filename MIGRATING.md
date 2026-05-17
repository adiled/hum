# Migrating from clwnd (v0.24.x) → hum 0.3

> _the old `clwnd` opencode plugin world is gone. this is the map from what you had to what's now._

If you installed `clwnd` (the opencode plugin shipped through v0.24.19),
this is the guide for you. **0.3 isn't a point release — it's the
rename + reshape.** The project is now `hum`. The daemon is now Rust.
The opencode integration is no longer a bespoke plugin; it's opencode's
own `openai-compatible` provider pointed at hum's `openai-server`
nestling.

Written for humans and agents. There's no migration script — too much
changed to mechanically translate. Read, edit, restart.

## In one breath

| old (v0.24.x clwnd) | new (hum 0.3) |
|---|---|
| name: `clwnd` | name: `hum` |
| `clwnd.schema.json` | `hum.schema.json` |
| TS daemon in `daemon/` | Rust binary at `~/.local/bin/humd` |
| `~/.config/clwnd/clwnd.json` (flat keys) | `~/.config/hum/hum.json` (namespaced sections) |
| opencode plugin in `~/.config/opencode/opencode.json` under `plugin.clwnd` | opencode `provider.hum` block (native `openai-compatible`) |
| `clwnd` socket — `<runtime>/clwnd/clwnd.sock` (and variants) | `thrum.sock` — `<runtime>/hum/thrum.sock` |
| projects registry in config | gone — `fs.roots` instead; nestlings own project notions if they need them |
| drone TS halves (`drone/llm.ts`, `cup.ts`, prompts) | drone is Rust-only; regex bank in `perches/common` |
| OC plugin shipped with hum repo | shed entirely; opencode talks to hum via plain openai-compat |

## Three-step upgrade

1. **Stop the old daemon, uninstall the plugin.**

   ```bash
   systemctl --user stop clwnd 2>/dev/null || true
   systemctl --user disable clwnd 2>/dev/null || true
   rm -f ~/.config/systemd/user/clwnd.service
   systemctl --user daemon-reload

   # opencode plugin
   rm -rf ~/.opencode/plugins/clwnd 2>/dev/null || true
   ```

   Then edit `~/.config/opencode/opencode.json` and **remove** any
   `plugin.clwnd` block (or whatever name the plugin shipped under).

2. **Install hum 0.3.**

   ```bash
   git clone https://github.com/adiled/hum.git
   cd hum
   ./install
   ```

   The installer:
   - builds the Rust `humd` binary
   - mints an Ed25519 identity at `~/.local/state/hum/humd.key`
   - writes a default `~/.config/hum/hum.json` (the new namespaced shape)
   - seeds an empty `~/.config/hum/peers.json`
   - writes the per-nestling-kind config `~/.config/hum/nestlings/openai-server.json`
   - installs the systemd user unit
   - builds the `openai-server` nestling (TS, ships in `nestlings/openai-server/`)
   - starts the daemon

3. **Edit `~/.config/opencode/opencode.json`** to add the new provider
   block (replaces the old `plugin.clwnd` block):

   ```jsonc
   {
     "provider": {
       "hum": {
         "type": "openai-compatible",
         "baseURL": "http://127.0.0.1:14620/v1",
         "models": {
           "claude-sonnet-4-5": {},
           "claude-haiku-4-5":  {}
         }
       }
     }
   }
   ```

   Then start the `openai-server` nestling (the install script builds
   it but does not yet auto-start it; a future commit adds the
   systemd unit). For now:

   ```bash
   node ~/.local/share/hum/src/nestlings/openai-server/dist/index.js &
   ```

   In opencode, pick `provider: hum`, model `claude-sonnet-4-5`. Done.

## hum.json — old key → new home

The old `clwnd.json` was flat. New `hum.json` is namespaced:

```jsonc
{
  "$schema": "https://adiled.github.io/hum/hum.schema.json",
  "humd": { "permissionDuskMs": 60000, "driftRetentionDays": 30 },
  "fs": {
    "roots": [{ "path": "~/code", "mode": "rw" }, { "path": "/tmp", "mode": "rw" }],
    "denied": ["~/.ssh", "~/.aws", "~/.gnupg", "~/.config/hum"]
  },
  "nest": { "maxProcs": 4, "idleThresholdMs": 300000, "default": "claude-cli" },
  "perches": {
    "claude-cli":  { "cliPath": "claude", "defaultModel": "claude-sonnet-4-5" },
    "claude-repl": { "cliPath": "claude", "defaultModel": "claude-sonnet-4-5" }
  }
}
```

| old key (clwnd.json) | new home (hum.json) |
|---|---|
| `maxProcs` | `nest.maxProcs` |
| `idleTimeout` | `nest.idleThresholdMs` (same unit, renamed) |
| `permissionDusk` | `humd.permissionDuskMs` |
| `nest` (enum `"claude-cli"` / `"claude-repl"`) | `nest.default` (free-form string; must appear in `perches`) |
| `driftRetentionDays` | `humd.driftRetentionDays` |
| `projects` | **dropped** — filesystem is now `fs.roots`; project IDs are a nestling-side concern |
| `droned` | **dropped** — drone is always on; the swallow path is internal |
| `droneModel` | **dropped** — the LLM-judge seam is in code (`drone::Evaluator` trait); no default judge ships |
| `smallModel` | **dropped** — title generation was a plugin concern; opencode handles it natively |
| `ccFlags` | `perches.claude-cli.ccFlags` (per-perch, not global) |
| `experimental.subpath` | **dropped** — plugin-only feature |
| `compaction` | **dropped** — manual-compaction is opencode's choice; hum doesn't proxy |
| `nestlings.<name>` (never shipped) | per-kind config at `~/.config/hum/nestlings/<name>.json`; humd doesn't pre-know nestlings |

## fs is new and worth understanding

The biggest single addition is the `fs` section — humd's filesystem
**capability primitive**. Built-in tools (Read/Write/Edit/Glob/Grep/Bash)
clamp to `fs.roots`; `fs.denied` overrides any root. `mode: "rw" | "ro"`
per root. The `SpawnSpec.cwd` for any spawned roost must sit inside
some root. Empty `roots` = humd has no fs access — useful for
inference-only nests.

## Nestling config lives outside hum.json

`~/.config/hum/nestlings/<kind>.json` per nestling kind. The
installer seeds `openai-server.json`:

```jsonc
{
  "host": "127.0.0.1",
  "port": 14620,
  "apiKey": ""
}
```

Each nestler reads its own file at startup, plus its own env
namespace (`OPENAI_SERVER_PORT`, `OPENAI_SERVER_HOST`,
`OPENAI_SERVER_API_KEY`). Precedence: env > config file > built-in
defaults. humd never reads these.

## Socket path

Old: `<runtime>/clwnd/clwnd.sock` (and variants).
New: `<runtime>/hum/thrum.sock` (canonical per WIRE.md). Env override:
`HUM_THRUM_SOCK`. Legacy `HUM_SOCKET` is accepted through 0.3 as a
fallback; will be removed in 0.4.

## MCP

opencode v2 speaks MCP natively. If you were relying on the clwnd
plugin to expose hum tools via MCP: that path is gone. Configure your
opencode session's MCP servers directly in opencode.json.

humd still has an embedded MCP server for its own tool surface;
spawned roosts get its URL via `SpawnSpec.mcp_url`. Each perch wires
it into the LLM's MCP config. The crate isn't gone, it's just no
longer bridging to a clwnd plugin.

## drone

The clwnd plugin had a TS-side drone with an LLM judge. That whole
half is gone — drone now lives entirely in the Rust `drone` crate
inside humd. Drone is **on by default**; no opt-out at the config
layer.

The regex pattern bank (heuristic gate) lives in
[`perches/common`](perches/common). Concrete `Perch` impls
(`claude-cli`, `claude-repl`) can register the regex `Classifier` if
they want context-loss swallow behavior. The
[`drone::Evaluator`](drone) trait is the seam for a future LLM judge
— host code can plug one in.

To effectively disable drone behavior: don't register a Classifier
(the default `NoopClassifier` flags nothing). Verdict never reaches
`Swallow`.

## What about `clwnd` the name?

Gone. The repo, the binary, the systemd unit, the opencode plugin,
the config dir — all renamed `hum`. Safe to delete:

- `~/.opencode/plugins/clwnd/`
- `~/.config/clwnd/`
- `~/.local/state/clwnd/`
- `~/.local/share/clwnd/`
- `~/.config/systemd/user/clwnd.service`
- `plugin.clwnd` block in `~/.config/opencode/opencode.json`

## If something doesn't start

- Config crashes at boot: shouldn't happen — the Rust loader is
  schema-tolerant. Unknown keys at any level warn and fall through to
  defaults. If you see a hard error, file an issue with your
  `hum.json` + the warn line from `./install logs`.
- Socket isn't there: check the systemd unit's env. The unit pins
  `HUM_THRUM_SOCK` to `<XDG_RUNTIME_DIR>/hum/thrum.sock`.
- opencode doesn't see hum: confirm `openai-server` is running on
  port 14620 (`curl http://127.0.0.1:14620/v1/models`), then confirm
  the `provider.hum` block is in opencode.json.

## See also

- `README.md` — what hum is.
- `VOCABULARY.md` — every load-bearing word.
- `WIRE.md` — the thrum protocol cold.
- `contracts/README.md` — on-chain primitives if you're interested.
- [adiled.github.io/hum](https://adiled.github.io/hum/) — docs site mirror.

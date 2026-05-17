# Migrating from 0.2 → 0.3

> _the old plugin world is gone. the new world is smaller and louder. this is the map._

hum 0.3 sheds the opencode plugin entirely. The provider boundary is
now **opencode's built-in `openai-compatible` adapter**, pointed at
hum's `openai-server` nestling. No bespoke plugin. No drift with
opencode's plugin API. No `hum.json` keys for plugin-only settings.

This guide is written for both humans and agents. Read it, then
`rm -rf ~/.config/hum` if you want a clean slate.

## What changed in one breath

| old (0.2) | new (0.3) |
|---|---|
| OC plugin in `nestlings/opencode/` | gone |
| OC consumes hum via plugin's `provider.ts` | OC consumes hum via its native `openai-compatible` provider, baseURL = `http://localhost:14620/v1` |
| `clwnd` plugin name in opencode.json | replaced by `provider.hum` (openai-compatible) |
| flat `hum.json` (`maxProcs`, `droned`, `ccFlags`, ...) | namespaced `hum.json` (`humd`, `fs`, `nest`, `perches`, `nestlings`) |
| `projects[]` registry at root | gone — nestlings handle their own project notion if they need one |
| drone runtime split (Rust core + TS plugin half) | drone is Rust-only; the regex pattern bank lives in `perches/common` |
| `~/.config/hum/hum.json` projects section | replaced by `fs.roots` (filesystem primitive at humd level) |

## Three-step upgrade

1. **Install or upgrade hum.** From the repo: `./install`. Or `hum update`.
2. **Edit `~/.config/opencode/opencode.json`.** Replace any `plugin.hum` or `clwnd` block with:

   ```jsonc
   {
     "provider": {
       "hum": {
         "type": "openai-compatible",
         "baseURL": "http://localhost:14620/v1",
         "models": {
           "claude-sonnet-4-5": {},
           "claude-haiku-4-5": {}
         }
       }
     }
   }
   ```

3. **Edit `~/.config/hum/hum.json`** down to the new shape (see below). Most of your old keys are gone; what's left has new homes.

That's it. Start humd (`systemctl --user start hum` or `hum start`), start the nestling (`hum-openai-server` — comes bundled), point OC at `provider: hum`, prompt away.

## The new hum.json, fully spelled out

```jsonc
{
  "$schema": "https://adiled.github.io/hum/hum.schema.json",

  "humd": {
    "permissionDuskMs": 60000,
    "driftRetentionDays": 30
  },

  "fs": {
    "roots": [
      { "path": "~/code", "mode": "rw" },
      { "path": "/tmp",    "mode": "rw" }
    ],
    "denied": ["~/.ssh", "~/.aws", "~/.gnupg", "~/.config/hum"]
  },

  "nest": {
    "maxProcs": 4,
    "idleThresholdMs": 300000,
    "default": "claude-repl"
  },

  "perches": {
    "claude-cli":  { "cliPath": "claude", "defaultModel": "claude-sonnet-4-5" },
    "claude-repl": { "cliPath": "claude", "defaultModel": "claude-sonnet-4-5" }
  }
}
```

Five sections, all optional. Missing section = `Default`. Unknown keys
are ignored (loader is tolerant; a stale field doesn't crash startup).

## Where nestling config lives

Not in `hum.json`. Each nestling is a separate process that owns its
own config. The install script seeds one file per nestling kind under
`~/.config/hum/nestlings/<kind>.json` — e.g.:

```jsonc
// ~/.config/hum/nestlings/openai-server.json
{
  "host": "127.0.0.1",
  "port": 14620,
  "apiKey": ""
}
```

Each nestler reads its own file at startup, plus its own env namespace
(`OPENAI_SERVER_PORT`, `OPENAI_SERVER_HOST`, …). humd discovers each
nestler when it sends `chi:"hello"` — there's no preconfigured list
of nestlings humd needs to know about.

## Old key → new home

If you had it in 0.2, this is where it lives now (or doesn't):

| old key | new location | notes |
|---|---|---|
| `maxProcs` | `nest.maxProcs` | semantic unchanged |
| `idleTimeout` | `nest.idleThresholdMs` | renamed; same unit (ms) |
| `permissionDusk` | `humd.permissionDuskMs` | renamed |
| `nest` (enum `"claude-cli"`/`"claude-repl"`) | `nest.default` (string) | now a free-form perch name; must appear in `perches` |
| `driftRetentionDays` | `humd.driftRetentionDays` | semantic unchanged |
| `projects` | **dropped** | filesystem capability is now `fs.roots`; project IDs are a nestling concern |
| `droned` | **dropped** | drone is always on; the swallow path is internal |
| `droneModel` | **dropped** | the LLM-judge seam is in code (`drone::Evaluator` trait), not config; no default judge ships |
| `smallModel` | **dropped** | title generation was a plugin concern; opencode handles it natively |
| `ccFlags` | `perches.claude-cli.ccFlags` (or `claude-repl`) | per-perch now, not global |
| `experimental.subpath` | **dropped** | plugin-only feature |
| `compaction` | **dropped** | manual-compaction is opencode's choice; hum doesn't proxy it |
| `nestlings.<name>` | **moved** | per-nestling-kind config now at `~/.config/hum/nestlings/<name>.json`. humd does NOT preconfigure nestlings — each nestler reads its own config + sends `chi:"hello"` |

## fs is new and worth understanding

The biggest single addition is the `fs` section. It's the first-class
**filesystem capability** humd offers — the set of roots inside which
built-in tools (Read / Write / Edit / Glob / Grep / Bash) are allowed
to operate, and the set of paths that are hard-denied regardless of
roots.

- `mode: "rw"` = read + write allowed.
- `mode: "ro"` = Read / Glob / Grep allowed; Write / Edit refused.
- A path under any `denied` entry is refused even if it sits inside a
  root. Used for secrets directories (`~/.ssh`, `~/.aws`, etc.).
- `SpawnSpec.cwd` (the working dir a perch spawns into) must be
  inside some `roots` entry. humd refuses to spawn outside.

Empty `roots` = humd has no filesystem access at all. Useful for
inference-only nests that don't need to touch disk.

## "projects" is what nestlings carve

In 0.2 hum maintained a registry of project IDs. That existed to
bridge opencode's notion of "project" to a hum-native canonical
ULID. With the plugin gone, the bridge isn't needed.

If a nestling wants to track its own per-conversation context across
runs, it stores that mapping in its own files. humd is no longer in
the project-naming business. It is in the *filesystem-permitting*
business.

## What about MCP?

Opencode v2 speaks MCP natively. If you were relying on hum's MCP
crate to expose tools to the opencode plugin: that path is gone. The
new path:

- For tools your OC sessions need: configure them in opencode's MCP
  config directly.
- For tools that should live next to humd (and thus be reachable by
  every nestler humd serves, not just OC): humd still has an embedded
  MCP server. The address gets passed into `SpawnSpec.mcp_url`. Each
  perch wires it into the LLM's MCP config. The crate isn't going
  anywhere; it's just no longer the OC bridge.

## What about drone?

Drone is **on by default**. No opt-out at the config layer. The Rust
crate (`drone/`) is the sentinel; the TS half (the LLM judge in
`drone/llm.ts`, the cup buffer in `drone/cup.ts`, the prompts in
`drone/prompts.ts`) were OC-plugin coupling and are removed.

The regex pattern bank (the heuristic gate) lives in
[`perches/common`](perches/common). Concrete `Perch` impls
(`claude-cli`, `claude-repl`) can register the regex `Classifier` on
their nest if they want context-loss swallow behavior. The
[`drone::Evaluator`](drone) trait remains the seam for a future LLM
judge — host code can plug one in without touching the drone crate.

If you want drone effectively off: don't register a Classifier (the
default `NoopClassifier` flags nothing). The verdict will never reach
`Swallow`.

## What about `clwnd`?

`clwnd` ceases to exist as a name. It was the plugin's project name
inside opencode's plugin store. After this migration:

- `~/.opencode/plugins/clwnd/` — delete safely
- `clwnd` in `opencode.json` `plugin.*` — remove the block
- `provider.clwnd` if you had one — replace with `provider.hum` as above

## Where to ask if something's missing

- Code-level: read the crate's README under the workspace root.
- Vocabulary: [VOCABULARY.md](VOCABULARY.md) is the canonical glossary.
- Wire-level: [WIRE.md](WIRE.md) defines the thrum protocol cold.
- Docs site: [adiled.github.io/hum](https://adiled.github.io/hum/) mirrors the same content.

If your config crashes startup after upgrade: it shouldn't. The
loader is schema-tolerant — unrecognized keys at any level are
warned about and ignored. If you see a hard error, file an issue
with the contents of your `hum.json` and the warn line from `hum logs`.

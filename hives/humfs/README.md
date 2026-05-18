---
title: "humfs (Rust)"
description: "hum's native filesystem forager hive — symbol-aware tool surface (humfs_read, humfs_do_code, humfs_do_noncode, humfs_bash) translating chi:tool-call ↔ filesystem operations"
---

# humfs

> _hum's native filesystem forager hive — translates `chi:"tool-call"`
> ↔ filesystem operations, AST-grounded via tree-sitter_

humfs is a symbol-aware filesystem surface for hum bees:

- **`humfs_read`** — one tool for discovering, studying, and
  searching. Auto-detects path semantics (file | directory | glob).
  AST-backed symbol outlines for code; anchor outlines for configs
  and docs. Mutually exclusive modifiers: `symbol` (exact, dot-
  nested for nested members), `query` (fuzzy on symbol names),
  `pattern` (regex over content — code matches carry their
  enclosing function/class symbol). Path-agnostic on extension —
  extensionless files (Dockerfile, Makefile, LICENSE) and unknown
  extensions (.lock, .xyz) return content the same way.
- **`humfs_do_code`** — AST-grounded code authoring. Operations:
  `create` | `replace` (symbol-scoped or whole-file) |
  `insert_before` | `insert_after` | `delete`. The top-of-file
  import block is addressable as the synthetic `imports` symbol.
  Sub-symbol walks (`body`, `when`, `otherwise`, `loop`, `try`,
  `return`, `call`) compose with dots and disambiguate with `#N`.
  Vue SFCs expose sub-blocks via `script.<name>`, `template.<tag>`,
  `style.<class>`. Every write is re-parsed for syntax errors
  before it lands.
- **`humfs_do_noncode`** — linguistic-scope edits for non-code.
  Four scopes: `word` (format-agnostic token swap), `phrase`
  (structural name — JSON/YAML key, env var, markdown heading,
  TOML section — or exact text), `sentence` (smallest independent
  unit), `paragraph` (full block). Omit `replace` to delete the
  scope; no scope param creates/overwrites the whole file.
- **`humfs_bash`** — shell escape hatch for runtime work: tests,
  git, builds, package managers, language toolchains. File-
  inspection commands route back to `humfs_read`; file-writing
  commands route to `humfs_do_code` / `humfs_do_noncode`. Output
  capped at 30 KB per stream; default timeout 120 s.

## Architecture

humfs is a **forager hive** — same shape as any other thrum-attached
process (openai-server, anthropic-server, ollama-server). It dials
humd's thrum socket, says hello with `bee: ["forager"]` and its
advertised `tools: [...]`, then handles `chi:"tool-call"` tones humd
routes here by `toolName`. Results go back as `chi:"tool-result"`.

The full chain for an OC user editing a file via humd:

```
OC ─HTTP─► openai-server forager ─chi:prompt─► humd
   ─chi:prompt─► claude-cli-worker (worker hive) ─chi:tool-call(humfs_read)─►
   humd ─chi:tool-call─► humfs-forager (this hive) ─fs op─► disk
   → chi:tool-result back through the chain
```

Foragers calling foragers. One forager originates the tool-call
(because the worker's LLM decided to read a file); humd routes by
`toolName` to whichever hive advertised that tool; humfs executes
against its own disk.

## Disk scoping

Each humfs forager owns its `fs.roots` snapshot, read from its local
`hum.json` at boot. Tool calls that resolve outside roots are
rejected at the forager. Ensemble Paradigm 1 (foragers on different
machines) follows the same shape — each humfs sees its own local
disk under its own roots policy.

## How humd picks the fs surface for workers

When humfs is registered, humd writes the union of humfs's
advertised tool names into every worker bee's
`SpawnSpec.disallowed_tools`. The worker passes that through to its
underlying harness, which means the harness's built-in fs tools
stay dormant for the session — every fs intent goes through humfs.

## Status

P0 lands as a skeleton: thrum dial, hello, `chi:"tool-call"` loop,
ToolDef registry, four tool stubs. Implementations roll in across
P1–P7. `humfs_bash` and `humfs_read` (no-AST mode) are live as of
P2; the AST-aware modifiers and `humfs_do_code` arrive with P3–P6.

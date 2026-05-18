---
title: "humfs (Rust)"
description: "hum's native filesystem forager hive — symbol-aware tool surface (humfs_read, humfs_do_code, humfs_do_noncode, humfs_bash) translating chi:tool-call ↔ filesystem operations"
---

# humfs

> _hum's native filesystem forager hive — translates `chi:"tool-call"`
> ↔ filesystem operations, AST-grounded via tree-sitter_

The fs surface a hum bee gets is **not** Anthropic's `Read`/`Write`/
`Edit`. It's a different class:

- **`humfs_read`** — ONE filesystem tool. Discovers, studies, and
  searches. Auto-detects path semantics (file | dir | glob).
  AST-backed symbol outlines for code; anchor outlines for configs/
  docs. Mutually exclusive modifiers: `symbol`, `query` (fuzzy on
  names), `pattern` (regex over content, code matches carry
  enclosing symbol). No offset, no limit, no pagination.
- **`humfs_do_code`** — AST-grounded code authoring. Operations:
  `create` | `replace` (symbol-scoped OR whole-file) |
  `insert_before` | `insert_after` | `delete`. Synthetic `imports`
  symbol. Sub-symbol walks (`body`, `when`, `otherwise`, `loop`,
  `try`, `return`, `call`) composable + disambiguated with `#N`.
  Vue SFC sub-block syntax. Syntax-validates before write.
- **`humfs_do_noncode`** — linguistic-scope edits for non-code.
  Scopes: `word`, `phrase` (format-aware: JSON/YAML key, env var,
  markdown heading, TOML section), `sentence`, `paragraph`.
- **`humfs_bash`** — escape hatch. Hard-bans `ls/find/grep/cat/…`
  (post-unwrap) — agents are redirected to `humfs_read`.

## How it slots in

humfs is a **forager hive** — same architectural status as
openai-server, anthropic-server, or any other thrum-attached
process. The full chain for an OC user editing a file:

```
OC ─HTTP─► openai-server forager ─chi:prompt─► humd ─chi:prompt─►
   claude-cli-worker (worker hive) ─chi:tool-call(humfs_read)─►
   humd ─chi:tool-call─► humfs-forager (forager hive) ─fs op─►
   disk → chi:tool-result back through the chain
```

Foragers calling foragers. The openai-server forager originates a
tool-call (because claude inside the worker decided to read a
file); humd routes by `toolName` to whichever hive advertised that
tool; humfs handles it against its own disk + fs.roots policy.

## Disk scoping

Each humfs forager owns its own `fs.roots` snapshot, read from its
local `hum.json` at boot. Tool calls that resolve outside roots
are rejected at the forager. Ensemble Paradigm 1 (foragers on
different machines) works the same way — each humfs sees its own
local disk, gated by its own roots.

## Workers' built-ins are blocked

When humfs is registered, humd populates every worker bee's
`SpawnSpec.disallowed_tools` with the union of humfs-advertised
tool names plus Anthropic's primitives (`Read`/`Write`/`Edit`/
`Bash`/`Glob`/`Grep`). Claude can't reach for its built-ins and
shadow humfs — only the humfs surface is available.

## Status

P0 lands as a skeleton: thrum dial, hello, `chi:"tool-call"` loop,
ToolDef registry, four tool stubs returning "not implemented".
Implementations roll in across P1–P7.

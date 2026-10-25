---
title: "codegen"
description: "Source-tree generator that emits consumer SDKs (TypeScript, Python, Go, C) from the Rust chi registry. No drift possible: every cargo build refreshes the clients."
---

# codegen

> _the Rust chi registry is canonical; this crate emits its TypeScript, Python, Go and C reflections so consumers in those languages cannot drift._

`thrum-core/src/chi.rs` is the only place a chi value is defined. The wire spec
([WIRE.md](../WIRE.md)) is the contract everybody else conforms to, and the chi
registry lives in Rust because the daemon and the in-process ensemble routing
already live in Rust. Anyone writing a bee in another language needs the same
list of chi values, the same `THRUM_VERSION`, the same views, and the same
client. This crate is what gives them that, by parsing the Rust as plain text
and emitting every file of every client under `thrum-clients/{ts,python,go,c}/`.
The client's behaviour is hum-thrum's, behind `hum bee link`, so what is
emitted per language is the registry, the views, the link's constants, and a
template that is only the transport to the link.

## Why a separate crate

Two reasons.

First, **no cycle on `thrum-core`**. If codegen depended on `thrum-core`, then
`thrum-core/build.rs` could not depend on codegen (cargo refuses circular
`[build-dependencies]`). So codegen parses `chi.rs` and `lib.rs` as text using
`regex`. It never `use`s `thrum-core`. That keeps the build graph clean.

Second, **the binary is for humans, the library is for build.rs**.
`cargo run -p codegen` is the manual lever for regenerating clients (and for
`--check` in CI). `thrum-core/build.rs` calls the same library functions on
every cargo build of `thrum-core`, so the clients refresh automatically as the
Rust enum changes. Same code path, two entry points.

## Layout

```
codegen/
├─ Cargo.toml
├─ templates/     each language's client: transport to `hum bee link`
└─ src/
   ├─ lib.rs      parse + the chi emitters, one per language
   ├─ protocol.rs the views, one emitter per language
   ├─ link.rs     the link's constants, rendered into the templates
   ├─ main.rs     CLI: per-target dispatch, --check drift gate
   └─ paths.rs    every literal repo path lives here (const + helper fn)
```

### `src/paths.rs`

Single source of truth for the repo layout codegen knows about. Two kinds of
items, paired:

```rust
pub const CHI_RS_REL: &str = "thrum-core/src/chi.rs";
pub fn chi_rs() -> PathBuf { repo_root().join(CHI_RS_REL) }
```

The `*_REL` constants are the repo-relative strings, used inside emitted
`@generated` headers (so the headers stay truthful through renames). The
`*()` functions resolve to absolute paths for disk I/O. `build.rs` and
`main.rs` both go through these. Nothing in the crate hand-writes
`"thrum-core/..."` or `"thrum-clients/..."` literals.

### `src/lib.rs`

Public surface:

| function | purpose |
|----------|---------|
| `parse(chi_rs, lib_rs)` | Read chi.rs + lib.rs, return a `ChiSpec` (version, chi variants, pulse variants, doc comments). |
| `emit_ts(spec, out)` | Write `chi.ts` to `out`. |
| `emit_py(spec, out)` | Write `chi.py` to `out`. |
| `emit_go(spec, out)` | Write `chi.go` to `out`. |
| `emit_c(spec, out)` | Write `chi.h` to `out`. |
| `link::link_spec(..)` | Read the link's version, exit codes and `hum` lookup from `hum-thrum/src/link.rs`, the client-owned hello fields from `hum-thrum/src/core.rs`, and the advisory codes from `chi.rs` with WIRE.md's advisories table. |
| `link::emit_client(spec, client, out)` | Write one client file: a template from `codegen/templates/<lang>/` with the link's constants at its `@constants` line. `paths::CLIENTS` lists them. |

All `emit_*` functions take a destination `Path`. Path resolution is the
caller's job: the canonical sites come from `paths::*`.

### `src/main.rs`

CLI dispatcher. No-arg invocation regenerates all three targets:

```bash
cargo run -p codegen              # ts + python + go
cargo run -p codegen -- ts        # one target
cargo run -p codegen -- --check   # exit non-zero if any target drifted
```

The `--check` path emits to a tempfile and byte-compares against the
checked-in artifact, so CI can gate on it without needing a separate
manifest of expected outputs.

## How a generated client gets refreshed

```
chi.rs (edit)
   │
   ▼
cargo build -p thrum-core
   │
   ├─ build.rs sees chi.rs changed
   ├─ calls codegen::parse(&paths::chi_rs(), &paths::lib_rs())
   └─ calls codegen::emit_{ts,py,go,c}, the protocol emitters and
      link::emit_client for every paths::CLIENTS entry,
      writing into thrum-clients/{ts,python,go,c}/
```

Every source file of every client is generated. The registry and the
views are read from thrum-core. The client itself is a template per
language, `codegen/templates/<lang>/`, holding only the transport to
`hum bee link`: spawn it, write lines, read lines, dispatch. It knows no
protocol, since the link is hum's Rust client, so a change to the wire
touches Rust and the regenerated files, never a template.

The TypeScript, Python, Go and C files are checked in (not gitignored). That
keeps consumers of those clients buildable without a Rust toolchain. CI
drift-checks them with `cargo run -p codegen -- --check` so a missed regen
fails the build instead of silently rotting.

## Output anatomy

Each generated chi file carries a header like:

```
// @generated by `cargo run -p codegen` from thrum-core/src/chi.rs — DO NOT EDIT.
//
// Rust is the canonical home of the wire registry. Hand-edit chi.rs;
// the file is regenerated on every cargo build of thrum-core (build.rs).
// Manual regen: `cargo run -p codegen`.
```

The source path in that header comes from `paths::CHI_RS_REL`, so a directory
rename updates every header on the next build. A client file's header names its
template and the Rust its constants came from, the same way.

## Adding a target

To add a new client language:

1. Add the path constants to `paths.rs`: `pub const FOO_CHI_REL: &str = "thrum-clients/foo/...";` plus the `foo_chi()` function.
2. Add `emit_foo(spec, out)` to `lib.rs` and a views emitter to `protocol.rs`.
3. Write the client template under `templates/foo/`, with an `@constants`
   line, add a `constants_foo` to `link.rs`, and list the files in
   `paths::CLIENTS`.
4. Add a match arm in `main.rs`'s `registry`, and the emitters to
   `thrum-core/build.rs`.
5. Write the language's echo bee and give `hum/tests/conform.rs` a way to
   run it, so the new client is held to the same behaviour as the rest.

After that, `cargo run -p codegen` writes the new artifacts and `--check`
gates drift on them.

## What this crate is not

- Not a parser for arbitrary Rust. It pattern-matches enum definitions and a
  small set of doc comment shapes. If you rewrite `chi.rs` with macros or move
  the enum to a different file, codegen breaks until updated. Keep `chi.rs`
  shaped the way the existing parser expects.
- Not a `thrum-core` consumer. By design, this crate has zero dependency on
  the Rust runtime types it reflects, so it can sit inside
  `thrum-core/build.rs`.

## See also

- [`thrum-core`](../thrum-core): the canonical home of the chi enum and
  `THRUM_VERSION`.
- [`thrum-clients/ts`](../thrum-clients/ts),
  [`thrum-clients/python`](../thrum-clients/python),
  [`thrum-clients/go`](../thrum-clients/go),
  [`thrum-clients/c`](../thrum-clients/c): the generated reflections.
- [WIRE.md](../WIRE.md): the language-neutral wire spec the clients implement.

---
title: "codegen"
description: "Reads hum.thrum, emits the TypeScript, Python, Go and C clients from it, and holds thrum-core's Rust to it. No drift possible: every cargo build refreshes the clients, and CI fails on a Rust that disagrees."
---

# codegen

> _`hum.thrum` is the thrum; this crate emits its TypeScript, Python, Go and C reflections and holds the Rust to it, so no copy can drift._

[`hum.thrum`](../hum.thrum), at the repo root, is the only place the thrum
is defined: the envelope, every chi with its body and who sounds and hears
it, the shapes and kinds bodies carry, the patterns tones make, and the
advisories. [WIRE.md](../WIRE.md) says what a party does with a tone; the
file says what a tone is. Anyone writing a bee in another language needs
the same range, the same `THRUM_VERSION`, the same views, and the same
client. This crate is what gives them that, by reading the file and
emitting every source file of every client under
`thrum-clients/{ts,python,go,c}/`. The client's behaviour is hum-thrum's,
behind `hum bee link`, so what is emitted per language is the range, the
views, the link's constants, and a template that is only the transport to
the link.

thrum-core's own Rust (`chi.rs`, `views.rs`, `envelope.rs`, `lib.rs`) is a
hand-written reading of the same file. codegen parses it as text and
compares: every chi, kind value and advisory code, every body field's key,
type and optionality, and the version. `cargo run -p codegen -- --check`
fails on any difference and names it in the file's own words;
`thrum-core/build.rs` warns.

## The `.thrum` language

One declaration per line, KEYWORD first. A DOC line documents the line
above it. A blank line is nothing; a line starting with `#` is a comment.
The grammar is the header of `hum.thrum` itself:

```
THRUM <version>                the range's version
ENVELOPE                       what every tone carries
CHI <name>                     one vibration in the range
  EMITS <party>..              who sounds it: a role, bee (any role), or humd
  HEARS <party>..              who it is for
  CARRIES ensemble             it may cross between humds
SHAPE <name>                   a shape nested in a body, not a chi of its own
KIND <name>                    a closed set of strings
  VALUE <name>                 one of them
FIELD <key> <type>             a wire key and its type; a trailing ? means optional
PATTERN <name>                 tones that make one figure
  KEY <field>                  what its tones share; the connection when absent
  OPEN / STEP / CLOSE <chi>..  how it opens, grows and closes
ADVISORY <code>                what the thrum tells a bee to fix, without refusing it
  BY <client|humd>..           who raises it
```

Types: `string`, `bool`, `int`, `uint`, `float`, `any`; `[t]` a list of
`t`; `{t}` a map of `t` keyed by string; a KIND or a SHAPE by its name;
`chi`, a chi of this range. Names are kebab-case, and each language's
spelling (`KadFindNodeResp`, `kadFindNodeResp`, `KAD_FIND_NODE_RESP`) is
derived from them. A wire key is written exactly as it goes on the wire.

`src/thrum.rs` reads the file into a `Thrum` and checks every rule the
grammar states: a type that names no KIND or SHAPE, a party that is no
role, a CHI without a DOC, a PATTERN that opens on no chi, a body that
declares a key twice. Each fault names its line or its item.

## Why a separate crate

Two reasons.

First, **no cycle on `thrum-core`**. If codegen depended on `thrum-core`,
then `thrum-core/build.rs` could not depend on codegen (cargo refuses
circular `[build-dependencies]`). So codegen reads `hum.thrum` itself and
parses thrum-core's Rust as text, `regex` for the enums and `syn` for the
structs. It never `use`s `thrum-core`. That keeps the build graph clean.

Second, **the binary is for humans, the library is for build.rs**.
`cargo run -p codegen` is the manual lever for regenerating clients (and
for `--check` in CI). `thrum-core/build.rs` calls the same library
functions on every cargo build of `thrum-core`, so the clients refresh
automatically as the file changes. Same code path, two entry points.

## Layout

```
codegen/
├─ Cargo.toml
├─ templates/     each language's client: transport to `hum bee link`
└─ src/
   ├─ thrum.rs    the reader: hum.thrum -> Thrum, and its projections
   ├─ rust.rs     the gate: thrum-core's Rust, held to the Thrum
   ├─ lib.rs      the chi emitters, one per language
   ├─ protocol.rs the views, one emitter per language; the Rust structs, parsed
   ├─ link.rs     the link's constants, rendered into the templates
   ├─ main.rs     CLI: per-target dispatch, --check drift gate
   └─ paths.rs    every literal repo path lives here (const + helper fn)
```

### `src/paths.rs`

Single source of truth for the repo layout codegen knows about. Two kinds of
items, paired:

```rust
pub const HUM_THRUM_REL: &str = "hum.thrum";
pub fn hum_thrum() -> PathBuf { repo_root().join(HUM_THRUM_REL) }
```

The `*_REL` constants are the repo-relative strings, used inside emitted
`@generated` headers (so the headers stay truthful through renames). The
`*()` functions resolve to absolute paths for disk I/O. `build.rs` and
`main.rs` both go through these. Nothing in the crate hand-writes
`"thrum-core/..."` or `"thrum-clients/..."` literals.

### `src/thrum.rs`

| function | purpose |
|----------|---------|
| `parse(path)` | Read `hum.thrum`, return a `Thrum`: version, envelope, chis, shapes, kinds, patterns, advisories. |
| `Thrum::chi_spec()` | The registries: the chis, and the `pulse-kind`, `petal-kind` and `role` kinds, each value with its DOC. |
| `Thrum::protocol_spec()` | The views: the envelope, one `<Chi>Body` per chi, one type per shape. A KIND-typed field is a string on the wire. |
| `Thrum::advisories()` | The advisory codes, with who raises each, for the link's constants. |

### `src/rust.rs`

`check(thrum, chi_rs, views_rs, envelope_rs, lib_rs)` compares the enums in
`chi.rs` (`Chi`, `PulseKind`, `PetalKind`, `Role`, `AdvisoryCode`), the
structs in `views.rs` and `envelope.rs`, and `THRUM_VERSION` in `lib.rs`
with the file, and returns every difference at once:

```
the Rust disagrees with hum.thrum:
  chi `backfill-event` is not in enum Chi (thrum-core/src/chi.rs)
  `PromptBody.season` is `uint?` in hum.thrum, `int?` in thrum-core/src/views.rs
```

### `src/lib.rs`

| function | purpose |
|----------|---------|
| `emit_ts(spec, out)` | Write `chi.ts` to `out`. |
| `emit_py(spec, out)` | Write `chi.py` to `out`. |
| `emit_go(spec, out)` | Write `chi.go` to `out`. |
| `emit_c(spec, out)` | Write `chi.h` to `out`. |
| `protocol::emit_protocol_{ts,py,go,c}(proto, out)` | Write the views. |
| `link::link_spec(link_rs, core_rs, thrum)` | Read the link's version, exit codes and `hum` lookup from `hum-thrum/src/link.rs`, the client-owned hello fields from `hum-thrum/src/core.rs`, and the advisories from the `Thrum`. |
| `link::emit_client(spec, client, out)` | Write one client file: a template from `codegen/templates/<lang>/` with the link's constants at its `@constants` line. `paths::CLIENTS` lists them. |

All `emit_*` functions take a destination `Path`. Path resolution is the
caller's job: the canonical sites come from `paths::*`.

### `src/main.rs`

CLI dispatcher. No-arg invocation regenerates all four targets:

```bash
cargo run -p codegen              # ts + python + go + c
cargo run -p codegen -- ts        # one target
cargo run -p codegen -- --check   # exit non-zero if the Rust or any target drifted
```

Every run holds the Rust to the file first and stops on a difference: a
client emitted while the Rust disagrees would be a client of two thrums.
The `--check` path then emits to a tempfile and byte-compares against the
checked-in artifact, so CI can gate on it without needing a separate
manifest of expected outputs.

## How a generated client gets refreshed

```
hum.thrum (edit)
   │
   ▼
cargo build -p thrum-core
   │
   ├─ build.rs sees hum.thrum changed
   ├─ codegen::thrum::parse reads it
   ├─ codegen::rust::check compares chi.rs, views.rs, envelope.rs, lib.rs
   │     (a difference is a cargo warning here; CI fails on it)
   └─ the emitters write thrum-clients/{ts,python,go,c}/
```

Every source file of every client is generated. The registry and the
views are projections of the file. The client itself is a template per
language, `codegen/templates/<lang>/`, holding only the transport to
`hum bee link`: spawn it, write lines, read lines, dispatch. It knows no
protocol, since the link is hum's Rust client, so a change to the thrum
touches the file, the Rust and the regenerated files, never a template.

The TypeScript, Python, Go and C files are checked in (not gitignored). That
keeps consumers of those clients buildable without a Rust toolchain. CI
drift-checks them with `cargo run -p codegen -- --check` so a missed regen
fails the build instead of silently rotting.

## Changing the thrum

A new chi, field, kind value or advisory starts in `hum.thrum`. Then the
Rust follows: the variant in `chi.rs`, the struct or field in `views.rs`,
the arm in `spec.rs`. Until both agree, `cargo build -p thrum-core` warns
and `cargo run -p codegen -- --check` fails, naming what differs. The
clients follow on the next build.

## Output anatomy

Each generated chi file carries a header like:

```
// @generated by `cargo run -p codegen` from hum.thrum — DO NOT EDIT.
//
// hum.thrum is the thrum. Edit it, and this file is regenerated on
// every cargo build of thrum-core (build.rs). Manual regen:
// `cargo run -p codegen`.
```

The source path in that header comes from `paths::HUM_THRUM_REL`, so a
rename updates every header on the next build. A client file's header
names its template and the Rust its constants came from, the same way.

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

- Not a parser for arbitrary Rust. The gate pattern-matches enum
  definitions and `serde` struct shapes. If you rewrite `chi.rs` with
  macros or move the enums to a different file, the gate breaks until
  updated. Keep thrum-core's Rust shaped the way the parser expects.
- Not a `thrum-core` consumer. By design, this crate has zero dependency on
  the Rust runtime types it checks, so it can sit inside
  `thrum-core/build.rs`.

## See also

- [`hum.thrum`](../hum.thrum): the thrum.
- [`thrum-core`](../thrum-core): its Rust reading, and `THRUM_VERSION`.
- [`thrum-clients/ts`](../thrum-clients/ts),
  [`thrum-clients/python`](../thrum-clients/python),
  [`thrum-clients/go`](../thrum-clients/go),
  [`thrum-clients/c`](../thrum-clients/c): the generated reflections.
- [WIRE.md](../WIRE.md): what a party does with a tone.

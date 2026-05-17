---
title: "thrum (Go)"
description: "wire-protocol primitives for Go nestlings"
---

# thrum (Go)

> _wire-protocol primitives for Go nestlings_

Go reference client for **thrum** — the NDJSON socket between humd
and nestlings. Same wire as Rust (`thrum-core`), TS (`thrum`), and
Python (`clients/python`); same algorithms; one source of truth.

## Install

```bash
go get github.com/adiled/hum/clients/go/thrum
```

(Subdirectory go modules — works today via Go's module resolution.)

## Quickstart

```go
package main

import (
    "context"
    "log"

    thrum "github.com/adiled/hum/clients/go/thrum"
)

func main() {
    c := thrum.NewClient("") // default socket path
    ctx := context.Background()
    if err := c.Connect(ctx); err != nil {
        log.Fatal(err)
    }
    defer c.Close()

    // Handshake. Optional fields (chi, propensity, source) feed the
    // on-mesh nestling registry — see ensemble/README.md.
    if err := c.Send(thrum.Tone{
        "chi":          string(thrum.ChiHello),
        "rid":          thrum.Rid(),
        "from":         "my-go-nestling",
        "nestling":     "my-go-nestling",
        "version":      "0.1.0",
        "protoVersion": thrum.ThrumVersion,
        "propensity": map[string]any{
            "statefulness": "stateless",
            "richness":     "lean",
            "wire":         "custom",
        },
        "chis": []string{
            string(thrum.ChiHello),
            string(thrum.ChiPrompt),
            string(thrum.ChiChunk),
            string(thrum.ChiFinish),
        },
    }); err != nil {
        log.Fatal(err)
    }

    c.On("hum-x", func(t thrum.Tone) {
        if chi, _ := t["chi"].(string); chi == string(thrum.ChiChunk) {
            log.Println(t["part"])
        }
    })
    c.OnAny(func(t thrum.Tone) { /* breath, echo, pulse */ })

    if err := c.Run(ctx); err != nil {
        log.Fatal(err)
    }
}
```

## What you get

| symbol | what |
|---|---|
| `Chi`, `ChiHello`, `ChiPrompt`, … | typed wire values |
| `PulseKind` | `pulse.kind` enum |
| `ThrumVersion` | "0.7.0" today |
| `AllChi`, `IsValidChi(s)` | membership checks |
| `Sigil(sid, nest)` | 12-char content hash |
| `Rid()` | monotonic correlation id |
| `DuskIn(ms)`, `IsDusk(t)` | expiry helpers |
| `WaneTracker` | Lamport clock per sigil |
| `DefaultSocketPath()` | resolves `$HUM_THRUM_SOCK` → XDG → `/run/user/<uid>` |
| `Client` | Unix-socket client; ~160 LoC |

## Sourced from

`chi.go` and `helpers.go` are **generated** from the canonical Rust
source (`thrum-core/src/chi.rs`) by `cargo run -p codegen`. Don't
hand-edit them. `client.go` is hand-written.

## See also

- [WIRE.md](../../thrum/WIRE.md) — the language-neutral protocol spec.
- [`thrum`](../../thrum) — TypeScript reference client.
- [`thrum-core`](../../thrum-core) — Rust reference client.
- [`clients/python`](../python) — Python reference client.
- [adiled.github.io/hum](https://adiled.github.io/hum/) — docs site.

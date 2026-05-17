---
title: "thrum (Python)"
description: "wire-protocol primitives for Python nestlings"
---

# thrum (Python)

> _wire-protocol primitives for Python nestlings_

Python reference client for **thrum** — the NDJSON socket between
humd and nestlings. Same wire as Rust (`thrum-core`) and TS
(`thrum`); same algorithms; one source of truth.

## Install

```bash
# Once published to PyPI:
pip install thrum

# Today (editable / git):
pip install git+https://github.com/adiled/hum.git#subdirectory=clients/python
# or
pip install -e ./clients/python
```

## Quickstart

```python
import asyncio
from thrum import Chi, ThrumClient, THRUM_VERSION, rid

async def main():
    c = ThrumClient()
    await c.connect()

    # Handshake. Optional fields (chi, propensity, source) feed the
    # on-mesh nestling registry — see ensemble/README.md.
    await c.send({
        "chi": Chi.HELLO,
        "rid": rid(),
        "from": "my-py-nestling",
        "nestling": "my-py-nestling",
        "version": "0.1.0",
        "protoVersion": THRUM_VERSION,
        "propensity": {"statefulness": "stateless", "richness": "lean", "wire": "custom"},
        "chis": [Chi.HELLO, Chi.PROMPT, Chi.CHUNK, Chi.FINISH],
    })

    async def on_hum_x(tone):
        if tone.get("chi") == Chi.CHUNK:
            print(tone.get("part"))

    c.on("hum-x", on_hum_x)
    c.on_any(lambda t: None)  # ignore tones without a sid

    await c.run_forever()

asyncio.run(main())
```

## What you get

| symbol | what |
|---|---|
| `Chi` | every wire-known chi value as a SCREAMING_SNAKE class constant |
| `PulseKind` | `pulse.kind` enum |
| `THRUM_VERSION` | "0.7.0" today; bump rules in [WIRE.md](../../thrum/WIRE.md) |
| `ALL_CHI`, `is_valid_chi(s)` | membership checks |
| `sigil(sid, nest)` | 12-char content hash |
| `rid()` | monotonic correlation id |
| `dusk_in(ms)`, `is_dusk(tone)` | expiry helpers |
| `WaneTracker` | Lamport clock per sigil |
| `default_socket_path()` | resolves `$HUM_THRUM_SOCK` → XDG → `/run/user/<uid>` |
| `ThrumClient` | async Unix-socket client; ~120 LoC |

## Sourced from

`chi.py` and `helpers.py` are **generated** from the canonical Rust
source (`thrum-core/src/chi.rs`) by `cargo run -p codegen`. Don't
hand-edit them. The `__init__.py` and `client.py` are hand-written.

## See also

- [WIRE.md](../../thrum/WIRE.md) — the language-neutral protocol spec.
- [`thrum`](../../thrum) — TypeScript reference client.
- [`thrum-core`](../../thrum-core) — Rust reference client.
- [`clients/go`](../go) — Go reference client.
- [adiled.github.io/hum](https://adiled.github.io/hum/) — docs site.

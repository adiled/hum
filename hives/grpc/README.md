---
title: "grpc-bee"
description: "gRPC bridge to hum — bidi stream, every chi flows through; transport-only bee"
---

# grpc-bee

> _gRPC bridge to hum — bidi stream, every chi flows through; transport-only bee_

A **transport-only** bee. One service, one RPC:
`Stream(stream Tone) returns (stream Tone)`. Every tone humd emits
flows back over gRPC; every tone the gRPC client sends is forwarded
to humd. Nothing is translated — gRPC is the transport, thrum is
still the protocol.

Rust workspace member; built with [tonic](https://github.com/hyperium/tonic).

## Propensity

| statefulness | richness | wire shape | hides |
|---|---|---|---|
| transport-only | opaque | gRPC bidi `Stream(stream Tone)` | nothing — every chi flows through |

## Wire (proto)

```proto
service Hum {
  rpc Stream(stream Tone) returns (stream Tone);
}

message Tone {
  string chi  = 1;   // discriminator
  string sid  = 2;   // multiplex key
  string rid  = 3;   // correlation
  bytes  body = 4;   // JSON-encoded full tone (authoritative)
}
```

`body` carries the full JSON tone (the same bytes humd sees on its
NDJSON socket). `chi` / `sid` / `rid` are pulled out so the bridge
can route without parsing the body on every frame.

## Prerequisites

`tonic-build` invokes `protoc` at build time. Install it once:

```bash
# Debian / Ubuntu
sudo apt-get install -y protobuf-compiler

# macOS
brew install protobuf
```

## Run

```bash
# From the workspace root.
cargo run -p grpc-bee

# Listen on a different addr:
HUM_GRPC_HOST=127.0.0.1 HUM_GRPC_PORT=14621 cargo run -p grpc-bee

# Point at a non-default humd socket:
HUM_THRUM_SOCK=/path/to/thrum.sock cargo run -p grpc-bee
```

Each gRPC bidi stream opens its own thrum connection so concurrent
clients can use overlapping sids without colliding handler state.
On stream open, the bee sends a `chi:"hello"` to humd which
gossips a `HiveManifest` to the rest of the ensemble — other
humds discover this bee via:

```rust
let mut found = ensemble.hive_discover("grpc");
```

## What it doesn't do

- **No TLS termination.** Plain h2. Front with envoy / nginx if you
  need it.
- **No auth.** Anyone who reaches the port can stream. Layer your own
  policy.
- **No translation.** A gRPC client that wants OpenAI-shaped chat
  completions should use the `openai-server` bee, not this.

## See also

- [`thrum-core`](../../thrum-core) — the wire contract.
- [`paid-oracle`](../paid-oracle) — another Rust bee reference
  (lean, x402-gated).
- [adiled.github.io/hum](https://adiled.github.io/hum/) — docs site.

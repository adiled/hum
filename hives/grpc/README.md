---
title: "grpc-forager (Rust)"
description: "a forager: the thrum over one gRPC bidi stream, its tones carried whole both ways"
---

# grpc-forager

> _a forager: the thrum over one gRPC bidi stream, its tones carried whole both ways_

A gRPC client opens `hum.Hum/Stream` and speaks as this humd's grpc
forager for as long as the stream lasts. The tones the client sends go
to humd, and the tones humd sends the bee that are the stream's come
back, each whole in a `Tone`. Nothing is translated: gRPC is the medium, and the client speaks the forager's
side of the thrum itself ([THRUM.md, Roles](../../THRUM.md#roles)). A client
that wants OpenAI- or Anthropic-shaped requests uses
[`openai-server`](../openai-server) or
[`anthropic-server`](../anthropic-server) instead.

## Propensity

| statefulness | richness | wire shape | hides |
|---|---|---|---|
| `transport-only` | `opaque` | `grpc/bidi` | the hello, which it signs; humd's advisories |

Its hello names no `chis`, so humd sends it every chi.

## Wire

```proto
package hum;

service Hum {
  rpc Stream(stream Tone) returns (stream Tone);
}

message Tone {
  string chi  = 1;
  string sid  = 2;
  string rid  = 3;
  bytes  body = 4;
}
```

`body` is the whole tone as UTF-8 JSON, the same line humd sees on its
socket.

- **Out to humd.** The client puts the tone in `body`; `chi`, `sid` and
  `rid` are then ignored. A Tone with an empty `body` is sent as just its
  `chi`, with its `sid` and `rid` when they are not empty, which is
  enough for a `cancel`. A `body` that is not a JSON object with a `chi`
  is dropped, and the client is not told. A tone with no `rid` is given
  one, and humd's echo of it is kept back. A `hello` is not sent: every
  stream is the same bee, so its hello is the bridge's, and a `hello`
  with a `rid` is answered with an `echo` whose `ok` is false.
- **Back to the client.** The tones humd sends the bee that are the
  stream's, with `chi`, `sid` and `rid` copied out of `body` (empty when
  the tone has none): every `breath`, the `echo` of a `rid` the stream
  sent, every tone under a sid the stream has prompted, attached or
  reached under, and every other tone that has no sid. Never humd's
  advisories, nor the echoes of the rids the bridge gave and of its own
  hello.

A prompt, as its `body`:

```json
{"chi": "prompt", "sid": "chat-1", "rid": "r1", "modelId": "claude-sonnet-4-6", "text": "hello"}
```

Back come the `echo` of `r1`, the bloom's `unfurl`s under `chat-1`, and
its `wilt`, or an `error` when it breaks off. A prompt with no `modelId`
goes to any nurse with room. A tool the client puts in
the prompt's `tools` reaches back to it as a `tendril-reach`, which it
answers with a `tendril-result` carrying the same `callId`.

### The stream

- **Opening.** The first stream enters the hive's forager under the
  label `main` and links it to humd, and every stream after it shares
  that bee. Until humd admits it the stream is open and silent, and what
  the client sends waits. The first tone back is a `breath`, which says
  what the bee can reach: the models, scents and tools
  ([THRUM.md, What a forager can reach](../../THRUM.md#what-a-forager-can-reach)).
- **Refused.** When humd refuses the bee, or its hive is not installed,
  the stream ends with status `FAILED_PRECONDITION` and the reason as
  its message. A refusal of the bee once admitted ends every open stream
  the same way, and the next stream links again.
- **humd away.** The streams stay open and the bridge dials again. What
  a client sends meanwhile waits until humd is back, the blooms open are
  lost, and a fresh `breath` says the bee is admitted again. A stream
  whose client has closed its side ends with `UNAVAILABLE`, since the
  blooms it waited on are lost.
- **Closing.** When the client closes its side, the stream stays open
  until every bloom it prompted has wilted, or broken off with an
  `error` or an `echo` whose `ok` is false, then ends with status OK. A
  client whose prompts offer tools keeps its side open to answer their
  reaches.
- **Going away.** When the client cancels the call, or its connection
  drops, the stream ends at once, and the bridge sends a `cancel` for
  each bloom the stream prompted that no other stream holds.

Every stream is the same bee on one connection to humd, so humd knows
one forager however many clients stream, and every stream hears each
`breath`. A sid is not a stream's own: streams that prompt or attach
under one sid hear each other's blooms under it, and an echo goes to the
stream that last sent its rid. Give each conversation a sid of its own,
and match what comes back by `sid`.

## Configure

| env | default | what |
|---|---|---|
| `HUM_GRPC_HOST` | `127.0.0.1` | the IP address it listens on; `0.0.0.0` or `::` for every interface |
| `HUM_GRPC_PORT` | `14621` | the port it listens on |
| `HUM_THRUM_SOCK` | the socket humd publishes in `runtime.json`, else `$XDG_STATE_HOME/hum/thrum.sock` | the humd it links to |
| `RUST_LOG` | `info` | the log filter |

Set `HUM_GRPC_HOST` or `HUM_GRPC_PORT` in the environment of
`hum hive grpc install`, which hands them to the forager's service; an
empty value keeps the default. `HUM_THRUM_SOCK` and `RUST_LOG` take
effect when the binary is run by hand.

## Run

Building it needs `protoc` on the `PATH`:

```bash
brew install protobuf                     # macOS
sudo apt-get install -y protobuf-compiler # Debian, Ubuntu
```

```bash
hum hive grpc install                        # build it and keep it running; the first stream enters its bee
HUM_GRPC_HOST=0.0.0.0 hum hive grpc install  # on every interface; see No auth
```

By hand, with the hive installed on the humd it links to:

```bash
cd hives/grpc && cargo build --release --locked
HUM_GRPC_PORT=14622 ./target/release/grpc-forager
```

The server has no reflection, so a generic client such as `grpcurl`
needs `proto/hum.proto`, and sends `body` as base64 in its JSON.

## Test

```bash
cd hives/grpc && cargo test
```

The tests are the forager as a gRPC client meets it, against a real
humd: `tests/stream.rs` covers the opening breath and a bloom, a stream
that closes its side while blooms grow, a client gone mid-bloom, streams
sharing the bee, a refusal, and the loopback default; `tests/outlive.rs`
covers humd going away. `tests/common` holds a minimal client, which is
also the shortest example of streaming to the forager.

## What it doesn't do

- **No auth.** It listens on loopback unless `HUM_GRPC_HOST` says
  otherwise, and whoever reaches the port is this humd's forager: their
  prompts reach every nurse here and on trusted peers, every tool the
  attendants serve, and, with a `cwd` naming this humd, the shell of a
  [`bash`](../bash) builder here. Set another host only where no one
  else can reach the port.
- **No TLS.** It serves plaintext HTTP/2; put a proxy that terminates
  TLS in front of it.
- **A Tone the client sends is at most 4 MiB**, tonic's default; a
  larger one ends the stream.

## See also

- [THRUM.md](../../THRUM.md): the tones the client sends and hears
- [hives](../README.md): how a hive is installed and its bees entered
- [`bp7`](../bp7): another forager in Rust, which translates
- [tonic](https://github.com/hyperium/tonic): the gRPC server it is built on

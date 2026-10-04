---
title: "openai-server"
description: "OpenAI-compatible HTTP surface for hum"
---

# openai-server

> _A hybrid bee: an OpenAI-compatible HTTP surface in front of hum, **and** a worker that consumes OpenAI's API for hum's daemons._

One process, one hive kind, one thrum connection — but two directions.
The **forager** half puts an OpenAI-shaped `/v1/chat/completions` + `/v1/responses`
server in front of hum's local thrum socket: any tool or client that
speaks OpenAI wire can drive a hum daemon without knowing thrum
exists. The **worker** half accepts `chi:"prompt"` tones humd routes
to this bee by advertised model, calls the upstream OpenAI API, and
streams the reply back. The hive is a hybrid bee (`bee:["forager","worker"]`)
— it both speaks OpenAI wire to hum's clients AND consumes OpenAI's
API for hum's daemons.

## Propensity

| statefulness | richness | wire shape | hides |
|---|---|---|---|
| convention-stateful | medium | OpenAI `/v1/chat/completions` SSE | pulse, breath, drone, perf-mark, tendril, permission-ask, tool-meta |

Forager half: convention-stateful — no server-side hum tracking; the
OpenAI `user` field is a session continuation hint. Worker half:
stateless-per-call (OpenAI's API is stateless — every request carries
full history), with a per-sid session object so tool results resume the
thread.

## What it does

```
client                           openai-server                       humd
  │                                   │                                │
  │  POST /v1/chat/completions        │                                │
  ├──────────────────────────────────►│                                │
  │   { messages, model, stream }     │                                │
  │                                   │  chi:"hello"                   │
  │                                   ├───────────────────────────────►│
  │                                   │  chi:"prompt"                  │
  │                                   ├───────────────────────────────►│
  │                                   │  chi:"chunk" (text fragments)  │
  │                                   │◄───────────────────────────────┤
  │   data: {...}\n\n (SSE)           │                                │
  │◄──────────────────────────────────┤                                │
  │                                   │  chi:"finish"                  │
  │                                   │◄───────────────────────────────┤
  │   data: [DONE]\n\n                │                                │
  │◄──────────────────────────────────┤                                │
```

## Worker half (the inverse direction)

Same bee also answers `chi:"prompt"` tones humd routes to it. humd
matches `modelId` against the models this bee advertises; when a
caller asks for one, humd forwards the prompt right back here. The
worker calls the upstream OpenAI API and streams chunks back — so a
daemon in another hive can ask **this** hive to generate, and it
consumes OpenAI's real API.

```
humd                          openai-server                    OpenAI API
  │  chi:"prompt" (modelId)         │                                │
  ├───────────────────────────────►│                                │
  │                                │  POST /chat/completions        │
  │                                ├───────────────────────────────►│
  │                                │◄─────────────── SSE deltas ────┤
  │                                │                                │
  │  chi:"chunk" (text_delta)      │                                │
  │◄───────────────────────────────┤                                │
  │  chi:"finish" (usage)          │                                │
  │◄───────────────────────────────┤                                │
```

Tools loop exactly like the Rust workers: when OpenAI returns
`tool_calls`, the worker emits `chi:"tool-call"` tones instead of
finishing; humd routes them to the owning forager, which returns
`chi:"tool-result"`; the worker feeds the output back into the OpenAI
conversation and continues streaming. Everything gets hummed.

## Configure

| env | default | what |
|---|---|---|
| `OPENAI_SERVER_PORT` | `14620` | HTTP listen port |
| `OPENAI_SERVER_HOST` | `127.0.0.1` | HTTP listen host |
| `OPENAI_SERVER_API_KEY` | _(unset → no auth)_ | bearer token required on requests |
| `HUM_THRUM_SOCK` | `$XDG_RUNTIME_DIR/hum/thrum.sock` | humd's NDJSON socket |
| `OPENAI_API_KEY` | `OPENAI_SERVER_API_KEY` | upstream OpenAI bearer (worker half) |
| `OPENAI_API_BASE` | `https://api.openai.com/v1` | upstream OpenAI-compatible base URL (worker half) |

The bee's own kind (`openai-server`) is its env namespace —
`HUM_*` is reserved for hum-side knobs like `HUM_THRUM_SOCK`.

### Config file (optional)

Also reads `~/.config/hum/hives/openai-server.json` if present:

```json
{ "host": "127.0.0.1", "port": 14620, "apiKey": "secret" }
```

The worker bee is the source of truth for what models the hive can
serve. On startup the worker half calls the upstream OpenAI-compatible
spec's own model listing (`GET {OPENAI_API_BASE}/models`) and reports
the discovered ids on its `chi:"hello"` and on this server's
`/v1/models`. Nothing hive-side governs the list — no env, no config
seed. humd routes `chi:"prompt"` to this bee only when `modelId`
matches a discovered model, and the worker rejects unknown ids.

## Run

```bash
npm install
npm run build
npm start
```

Or in dev:

```bash
npx tsx src/index.ts
```

## Use

```bash
curl http://localhost:14620/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "sonnet",
    "stream": true,
    "messages": [{ "role": "user", "content": "ping" }]
  }'
```

Drop-in for the OpenAI SDK:

```ts
import OpenAI from "openai";
const client = new OpenAI({
  baseURL: "http://localhost:14620/v1",
  apiKey:  "anything",
});
const r = await client.chat.completions.create({
  model: "sonnet",
  messages: [{ role: "user", content: "ping" }],
});
```

## Status

Reference implementation. Tools / function-calling map onto thrum's
`chi:"tool-call"` / `chi:"tool-result"` pair; lots of OpenAI surface
(images, audio, fine-tuning) is intentionally not implemented — file
an issue if you need a specific endpoint.

## See also

- [`thrum`](../../thrum) — the npm package this bee imports.
- [`paid-oracle`](../paid-oracle) — for monetizing this bee
  via x402-style payment.
- [adiled.github.io/hum](https://adiled.github.io/hum/) — docs site.

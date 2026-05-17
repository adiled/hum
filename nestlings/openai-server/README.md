---
title: "openai-server"
description: "OpenAI-compatible HTTP surface for hum"
---

# openai-server

> _OpenAI-compatible HTTP surface for hum_

A nestling that puts an OpenAI-shaped `/v1/chat/completions` server in
front of hum's local thrum socket. Any tool, agent framework, or
client library that speaks OpenAI's chat-completions API can drive a
hum daemon without knowing thrum exists.

## Propensity

| statefulness | richness | wire shape | hides |
|---|---|---|---|
| convention-stateful | medium | OpenAI `/v1/chat/completions` SSE | pulse, breath, drone, perf-mark, tendril, permission-ask, tool-meta |

Convention-stateful: no server-side hum tracking; the OpenAI `user`
field is treated as a session continuation hint.

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

## Configure

| env | default | what |
|---|---|---|
| `HUM_OPENAI_PORT` | `8787` | HTTP listen port |
| `HUM_OPENAI_HOST` | `0.0.0.0` | HTTP listen host |
| `HUM_THRUM_PATH` | `$XDG_RUNTIME_DIR/hum/hum.sock.thrum` | humd's NDJSON socket |

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
curl http://localhost:8787/v1/chat/completions \
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
  baseURL: "http://localhost:8787/v1",
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

- [`thrum`](../../thrum) — the npm package this nestling imports.
- [`paid-oracle`](../paid-oracle) — for monetizing this nestling
  via x402-style payment.
- [adiled.github.io/hum](https://adiled.github.io/hum/) — docs site.

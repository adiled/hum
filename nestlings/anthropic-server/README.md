---
title: "anthropic-server"
description: "Anthropic-compatible HTTP surface for hum (POST /v1/messages, SSE streaming)"
---

# anthropic-server

> _Anthropic-compatible HTTP surface for hum (POST /v1/messages, SSE streaming)_

A nestling that fronts hum's local thrum socket with the Anthropic
Messages API. Drop-in for `@anthropic-ai/sdk` — point `baseURL` at
this server and existing Anthropic clients work against hum without
knowing thrum exists. Symmetric counterpart to
[`openai-server`](../openai-server).

## Propensity

| statefulness | richness | wire shape | hides |
|---|---|---|---|
| convention-stateful | medium | Anthropic `/v1/messages` SSE | pulse, breath, drone, perf-mark, tendril, permission-ask, tool-meta |

Each request mints a fresh sid; continuation flows through the
`messages` array as `tool_result` content blocks.

## What it does

```
client                           anthropic-server                    humd
  │                                     │                              │
  │  POST /v1/messages                  │                              │
  ├────────────────────────────────────►│                              │
  │   { model, messages, system,        │                              │
  │     tools, stream:true }            │  chi:"hello" on first call   │
  │                                     ├─────────────────────────────►│
  │                                     │  chi:"prompt" (text/system/tools)
  │                                     ├─────────────────────────────►│
  │                                     │  chi:"chunk" (text/tool_use) │
  │                                     │◄─────────────────────────────┤
  │  event: content_block_delta         │                              │
  │  data: { delta: { text: "Hi" } }    │                              │
  │◄────────────────────────────────────┤                              │
  │                                     │  chi:"finish"                │
  │                                     │◄─────────────────────────────┤
  │  event: message_stop                │                              │
  │◄────────────────────────────────────┤                              │
```

Tool use flows symmetrically: model tool_use blocks come out via
`chi:"tool-call"`, and the client's `tool_result` content blocks in
the next message come back as `chi:"tool-result"` carrying the
matching `callId`.

## Configure

| env | default | what |
|---|---|---|
| `ANTHROPIC_SERVER_PORT` | `14622` | HTTP listen port |
| `ANTHROPIC_SERVER_HOST` | `127.0.0.1` | HTTP listen host |
| `ANTHROPIC_SERVER_API_KEY` | (empty = no auth) | required `x-api-key` header value |
| `HUM_THRUM_SOCK` | `$XDG_RUNTIME_DIR/hum/thrum.sock` | humd's NDJSON socket |

You can also drop a JSON config at
`~/.config/hum/nestlings/anthropic-server.json` with shape
`{ host?, port?, apiKey? }`. Precedence: env > config file > defaults.

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
curl http://localhost:14622/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: anything" \
  -d '{
    "model": "claude-sonnet-4",
    "max_tokens": 1024,
    "stream": true,
    "messages": [{ "role": "user", "content": "ping" }]
  }'
```

Drop-in for the Anthropic SDK:

```ts
import Anthropic from "@anthropic-ai/sdk";
const client = new Anthropic({
  baseURL: "http://localhost:14622",
  apiKey:  "anything",
});
const stream = await client.messages.stream({
  model: "claude-sonnet-4",
  max_tokens: 1024,
  messages: [{ role: "user", content: "ping" }],
});
for await (const chunk of stream) console.log(chunk);
```

## What flows where

| Anthropic Messages surface | hum chi |
|---|---|
| POST `/v1/messages` | `chi:"prompt"` |
| `system` (string or `[{type:"text"}]`) | `prompt.systemPrompt` |
| `tools[]` (with `input_schema`) | `prompt.tools[]` |
| `tool_result` blocks in last user msg | `chi:"tool-result"` |
| `content_block_delta` text | `chi:"chunk"` (text part) |
| `content_block_delta` input_json | `chi:"chunk"` (tool_use partial) |
| `message_stop` | `chi:"finish"` |
| `error` event | `chi:"error"` |

## Status

Reference implementation. Streaming + non-streaming both supported.
Tool use is forwarded; image inputs, prompt caching, and vision are
not yet wired.

## See also

- [`openai-server`](../openai-server) — symmetric OpenAI surface.
- [`thrum`](../../thrum) — the npm package this nestling imports.
- [WIRE.md](../../WIRE.md) — the language-neutral protocol spec.
- [adiled.github.io/hum](https://adiled.github.io/hum/) — docs site.

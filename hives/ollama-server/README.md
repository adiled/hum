---
title: "ollama-server (Rust)"
description: "Ollama for hum, both ways: a drop-in surface for any Ollama client, and a nurse that calls Ollama"
---

# ollama-server (Rust)

> _Ollama for hum, both ways: a drop-in surface for any Ollama client, and a nurse that calls Ollama_

A bee that fronts hum's local thrum socket with the Ollama REST
API. Drop-in for `ollama`, `open-webui`, `lobe-chat`, `cline`,
LangChain's `ChatOllama`, or anything else that speaks Ollama's
`/api/chat` + `/api/generate`. Default port `11434` matches Ollama's
own default so most clients work with **zero config**.

Built in Rust with [axum](https://github.com/tokio-rs/axum). Workspace
member of the main hum repo.

## Propensity

| statefulness | richness | wire shape | hides |
|---|---|---|---|
| convention-stateful | medium | Ollama `/api/chat` + `/api/generate` NDJSON | pulse, breath, drone, perf-mark, tendril, tool-meta |

Ollama's streaming response is already line-delimited JSON — it
maps to thrum's frame format 1:1. No SSE re-framing.

## What it does

```
client                              ollama-server                     humd
  │                                       │                            │
  │  POST /api/chat                       │                            │
  ├──────────────────────────────────────►│                            │
  │   { model, messages, stream:true,     │                            │
  │     tools? }                          │  chi:"prompt"              │
  │                                       ├───────────────────────────►│
  │                                       │  chi:"unfurl"              │
  │                                       │◄───────────────────────────┤
  │  {"message":{"content":"Hi"},"done":false}                         │
  │◄──────────────────────────────────────┤                            │
  │                                       │  chi:"wilt"                │
  │                                       │◄───────────────────────────┤
  │  {"done":true,...}                    │                            │
  │◄──────────────────────────────────────┤                            │
```

## Endpoints

| route | what |
|---|---|
| `POST /api/chat` | multi-message chat; NDJSON streaming |
| `POST /api/generate` | single prompt; NDJSON streaming |
| `GET /api/tags` | list of available models (synthesized) |
| `GET /` | health probe — returns `"Ollama is running"` like the real Ollama |

## Configure

| env | default | what |
|---|---|---|
| `OLLAMA_SERVER_PORT` | `11434` | HTTP listen port (matches Ollama's default) |
| `OLLAMA_SERVER_HOST` | `127.0.0.1` | HTTP listen host |
| `OLLAMA_SERVER_MODELS` | `claude-sonnet-4,claude-haiku-4.5,claude-opus-4.7` | comma-separated list returned by `/api/tags` |
| `HUM_THRUM_SOCK` | `$XDG_STATE_HOME/hum/thrum.sock` | humd's NDJSON socket |

Optional per-kind config file at `~/.config/hum/hives/ollama-server.json`:

```json
{
  "host": "127.0.0.1",
  "port": 11434,
  "models": ["claude-sonnet-4", "claude-haiku-4.5", "claude-opus-4.7"]
}
```

Precedence: env > config file > built-in defaults.

## Run

```bash
# From the workspace root.
cargo run -p ollama-server

# Listen on a different port:
OLLAMA_SERVER_PORT=14620 cargo run -p ollama-server
```

## Use

Plain curl:

```bash
curl http://localhost:11434/api/chat \
  -H "Content-Type: application/json" \
  -d '{
    "model": "claude-sonnet-4",
    "stream": true,
    "messages": [{ "role": "user", "content": "ping" }]
  }'
```

Ollama JS client:

```ts
import { Ollama } from "ollama";

const ollama = new Ollama({ host: "http://localhost:11434" });
const r = await ollama.chat({
  model: "claude-sonnet-4",
  messages: [{ role: "user", content: "ping" }],
  stream: true,
});
for await (const chunk of r) process.stdout.write(chunk.message.content);
```

LangChain:

```python
from langchain_ollama import ChatOllama
llm = ChatOllama(model="claude-sonnet-4", base_url="http://localhost:11434")
print(llm.invoke("ping"))
```

## What flows where

| Ollama surface | hum chi |
|---|---|
| POST `/api/chat` | `chi:"prompt"` |
| messages[].role=system | `prompt.systemPrompt` |
| tools[] (function) | `prompt.tools[]` |
| streamed `message.content` | `chi:"unfurl"` (a text petal) |
| streamed `message.tool_calls` | `chi:"tendril-reach"` |
| `done:true` line | `chi:"wilt"` |
| `error` line | `chi:"error"` |

## The nurse half

The hive's second bee, `ollama-nurse`, is entered as a nurse under the
label `nurse` and goes the other way: it serves the models in
`OLLAMA_NURSE_MODELS` by calling Ollama at `OLLAMA_NURSE_URL` (default
`http://127.0.0.1:11434`). Both run from the one Orchfile. When the
forager holds port 11434 itself, point the nurse at Ollama's real
address; a call the nurse makes to its own forager is refused rather
than looped back into hum.

## Status

Reference implementation. Streaming + non-streaming both supported.
Tool use forwarded; embedding endpoints and image generation are not.

## See also

- [`openai-server`](../openai-server), [`anthropic-server`](../anthropic-server) — sibling surfaces.
- [`thrum`](../../thrum) — the npm package this bee imports.
- [THRUM.md](../../THRUM.md) — the Thrum Protocol.
- [adiled.github.io/hum](https://adiled.github.io/hum/) — docs site.

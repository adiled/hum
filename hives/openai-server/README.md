---
title: "openai-server"
description: "OpenAI's HTTP API in front of hum, and bees behind it that call an OpenAI-compatible API"
---

# openai-server

> _OpenAI's HTTP API in front of hum, and bees behind it that call an OpenAI-compatible API._

Any client that speaks OpenAI drives hum through this hive without
knowing hum exists: the official SDKs, coding agents, chat UIs, editor
completion. Its requests are grown by whatever nurse humd can reach, here
or on a trusted peer, and its embeddings distilled by whatever attendant
serves them. The hive also serves the API at `OPENAI_API_BASE` to hum, so
that API's models are grown and distilled for every other forager too.

## Its bees

| bee | role | does |
|---|---|---|
| `key-<name>`, one per key | forager | asks humd for each request a key's caller makes |
| `anonymous` | forager | the same, when no key exists |
| `main` | nurse | grows the upstream's models |
| `scent` | attendant | distils the upstream's embedding models |

## The surface

| route | |
|---|---|
| `POST /v1/chat/completions` | streamed or not, with tools, images, audio and files |
| `POST /v1/responses`, `GET` and `DELETE /v1/responses/{id}` | the Responses API, `previous_response_id` and `conversation` included |
| `POST /v1/completions` | text grown on from as it stands, `suffix` for filling in the middle |
| `POST /v1/embeddings` | floats or base64 |
| `GET /v1/models`, `/v1/models/{id}` | what the caller's bee can reach; a model that grows words lists, as `tools`, the tools the server runs itself |
| `GET /v1/usage` | what the calling key has taken |

Every route answers without its `/v1` too.

**Conversations.** OpenAI's APIs send the whole conversation every time;
hum keeps it in a sigil. A request's earlier turns travel as the prompt's
`graft`, so any nurse can grow it, and its last turn as the prompt. A
conversation stays on one sigil while it only grows, so a nurse that keeps
sessions picks up where it was; one whose history was edited, or one
already growing, gets a sigil of its own. Two conversations never share
one.

**Tools.** A caller's function tools are tools a bloom may reach back to
it for, by their exact names. Calls come back as `tool_calls`, or
`function_call` items, several at once when the model makes several, and
their answers carry the bloom on. A tool the server runs, `bash` where a
bloom builds or an attendant's, stays out of chat completions, which has
no place for a result the caller did not produce; a response shows it as
an `mcp_call` item, output and all.

**Failures.** An unknown model is a 404, a request the nurse cannot grow a
400, a nurse or upstream failure a 502, humd away a 503, a request past
its time a 504, each in OpenAI's error shape. A stream that has begun
ends with its error. A caller that hangs up cancels its bloom.

## Configure

| env | config key | default | |
|---|---|---|---|
| `OPENAI_SERVER_HOST` | `host` | `127.0.0.1` | where the surface listens |
| `OPENAI_SERVER_PORT` | `port` | `14620` | |
| `OPENAI_SERVER_API_KEY` | `apiKey` | | a key named `default` |
| `OPENAI_SERVER_TIMEOUT_MS` | `timeoutMs` | 15 minutes | how long one request may take |
| | `cors` | none | origins a browser may call from, or `"*"` |
| | `rate` | none | `{ "capacity": 60, "refillPerSec": 1 }` for each key |
| | `scents` | ids that say `embed` | the upstream models the `scent` attendant distils by |
| `OPENAI_API_BASE` | | `https://api.openai.com/v1` | the API the `main` and `scent` bees call |
| `OPENAI_API_KEY` | | | its key |

The config file is `hives/openai-server.json` under hum's config root.

## Keys

```bash
node dist/index.js key add alice     # prints alice's token, once
node dist/index.js key list
node dist/index.js key remove alice
```

A token is kept only as its sha256, in `secrets/keys.json` under the
hive's state, and a key counts as soon as it is added. Each key is one
forager bee, so humd hears every caller apart and `hum bee <id> exit` shuts
one out. With no key at all, anyone who reaches the port may call.

## Use

```ts
import OpenAI from "openai";
const client = new OpenAI({ baseURL: "http://127.0.0.1:14620/v1", apiKey: "sk-hum-..." });
const r = await client.chat.completions.create({
  model: "claude-opus-4-7",
  messages: [{ role: "user", content: "ping" }],
});
```

## Test

```bash
pnpm install && pnpm run build
cargo build -p hum -p humd           # from the repo root
bun test
```

The end-to-end tests run the official OpenAI SDK against this server
under Node, a humd of their own built from this checkout, and a scripted
upstream.

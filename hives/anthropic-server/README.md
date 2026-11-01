---
title: "anthropic-server"
description: "the Anthropic Messages API in front of hum (POST /v1/messages, one message or streamed as SSE)"
---

# anthropic-server

> _the Anthropic Messages API in front of hum: `POST /v1/messages`, as one message or streamed as SSE_

A client that speaks Anthropic's Messages API, `@anthropic-ai/sdk` among
them, points its base URL here and prompts hum without knowing the thrum
exists. Each request is one bloom. humd hands it to whichever nurse grows
the model, on this humd or on a trusted peer, and the bloom comes back
shaped as Anthropic's API shapes a message.

The hive is one bee, a forager entered as `main`, and every caller
prompts through it. It is narrower than [`openai-server`](../openai-server):
one bee for every caller, and the Messages API alone.

## Propensity

| statefulness | richness | wire shape | hides |
|---|---|---|---|
| convention-stateful | medium | `anthropic/messages` | every petal but text and sepal; the tools humd serves inside it |

Its hello lists the chis `hello`, `prompt`, `cancel`, `cleanup`, `unfurl`,
`wilt`, `error`, `tendril-reach` and `tendril-result`, and names where it
listens (`bind`), for the ensemble's manifest of it. It does not say
`builds`, and a request carries no `cwd`, so its blooms build nowhere: a
`bash` reach is answered as unserved.

## Wire

```
client ─POST /v1/messages─► anthropic-server ─chi:"prompt"─► humd ─► nurse, here or across the ensemble
client ◄──── message or SSE ─ anthropic-server ◄──── chi:"unfurl"s, then chi:"wilt" ────┘
```

| route | |
|---|---|
| `POST /v1/messages` | one bloom, as one message or streamed |
| `GET /v1/usage` | what the caller's tenant has taken |
| `GET /` | `hum-anthropic-server` |

A route is its path, and a query is not read, so the SDK's
`client.beta.messages`, which posts to `/v1/messages?beta=true`, is
served as `messages` is. Any other path is a 404.

## What flows where

| Anthropic Messages | thrum |
|---|---|
| `model` (required) | `prompt.modelId` |
| `system`, a string or text blocks, and the words of any message whose role is `system` | `prompt.systemPrompt`, joined by blank lines |
| the last user message's text, images and documents | `prompt.content`: text petals, and pollen |
| every message before it | `prompt.graft`, one bloom per user turn |
| `max_tokens` | `prompt.season` |
| `temperature` | `prompt.temperature` |
| `top_p`, `top_k` | `prompt.prune`, its `p` and `k` |
| `stop_sequences` | `prompt.frost` |
| `thinking.budget_tokens`, `output_config.effort` | `prompt.sepal`, its `budget` and `effort` |
| `output_config.format` of type `json_schema` | `prompt.trellis`, its schema |
| `tool_choice` | `prompt.tendrils`: `auto` and `none` as they are, `any` as `required`, a named tool as its name, `disable_parallel_tool_use` as `parallel: false` |
| `tools[]`, each custom tool with `input_schema` | `prompt.tools[]`, each with `inputSchema` |
| `tool_result` blocks that end the conversation, for reaches it holds | `chi:"tendril-result"` on the reaching bloom's sid: `callId` its `tool_use_id`, `output` its text, `isError` its `is_error` |
| a text block, `text_delta` | `chi:"unfurl"` of a text petal |
| a thinking block, `thinking_delta`, `signature_delta` | `chi:"unfurl"` of a sepal petal, its propolis the signature |
| a `tool_use` block, `stop_reason: "tool_use"` | `chi:"tendril-reach"` for a tool the request offered |
| `message_delta` with `stop_reason` and `usage`, then `message_stop` | `chi:"wilt"`, its `reason` and `nectar` |
| an `error` event | `chi:"error"` |
| a client that hangs up | `chi:"cancel"` |

**The conversation.** Each user turn opens a bloom of the graft. What the
assistant said is a text petal, its thinking a sepal sealed by its
signature, and each `tool_use` a tendril petal, `callId` its `id` and
`args` its `input`; each `tool_result` is a grasp of its text, `isError`
its `is_error`. A user message that holds both tool results and words
closes one turn and opens the next. A conversation that ends with tool
results, or with the assistant's own words, asks the bloom to grow on: the
prompt has no content, and every bloom is its graft.

**Images and documents** are pollen, one grain each. An image's base64
source is a grain of its media type with its bytes inline, and a url
source a grain at that url, of type `image/*`. A PDF's base64 or url
source is a `file` grain the same way, of type `application/pdf`. A
plain-text document is a `text/plain` file grain, its words the bytes,
and a document of content blocks is those blocks, as petals.

Nothing else in a request is sent: a Files API source, redacted thinking,
the blocks of Anthropic's server tools, search results, a tool result's
images and documents, tools of any type but custom, a thinking config
other than a budget, `cache_control`, `citations`, `metadata`,
`service_tier` and `container` are all left out.

**The sigil.** A conversation keeps one sigil while it grows, so a nurse
whose cell grew every bloom of the graft grows on from its own, and any
other nurse grows from the graft. A client may name its line with an
`X-Hum-Sid` header, kept apart from every other tenant's. Otherwise the
sigil is found by the newest tool use or result id the conversation
holds that its tenant was given, or, with no tools, by the words of its
last two turns. The whole history is not compared: clients compact it
all the time, dropping old tool pairs, trimming, or sending a turn back
without the words said between its tool uses, while a tool use's id
comes back as it was given. A client that edits an older turn keeps its
sigil, but a sigil goes on only from where it last left off, so a copy
of the conversation taken before then gets one of its own; one already
growing gets a sigil of its own, and tenants never share one. The server
keeps 4096 ids and 4096 tails, and sends `cleanup` for a sigil no key
finds any more.

**The answer.** A request streams only when its body says
`"stream": true`. A request without it gets one message: its text,
thinking and `tool_use` blocks in the order they opened. A stream is
Anthropic's events, its `message_start` held until the bloom first grows,
and a `ping` every 15 seconds while it is quiet.

`stop_reason` is `end_turn` for a wilt whose reason is `stop`, `success`
or absent, `max_tokens` for `length` and `max_tokens`, `tool_use` when the
bloom reached for the caller's tools, and the wilt's own reason otherwise.
`stop_sequence` is always null. `usage` is the wilt's nectar:
`input_tokens`, `output_tokens`, and the cache's
`cache_creation_input_tokens` and `cache_read_input_tokens` when the nurse
gave them.

**Tools.** A request's custom tools go on its prompt. When the bloom
reaches one, humd reaches the server, and the reach is a `tool_use` block,
its `id` the reach's `callId`. Reaches that come together come back
together, and once they stop coming the message ends with
`stop_reason: "tool_use"`. The server holds each reach for thirty minutes.
When the client's next request ends with `tool_result` blocks for held
reaches of one bloom, they go to that bloom as `chi:"tendril-result"`, no
prompt is sent, and what it grows on is that request's answer. A result
the server does not hold grows on from the graft instead, the tool uses
and results among its petals. A reach that comes after its turn went back
to the client is answered at once with an error asking the cell to call
again. humd also hands the bloom the tools attendants serve; a reach for
one of those is answered inside humd, and the client sees only the words
the bloom grows around it.

**Failures.**

| status | type | when |
|---|---|---|
| 400 | `invalid_request_error` | the body is not a JSON object, has no `model`, its `messages` is empty or not a list or holds nothing to answer, or the nurse cannot grow the prompt (`cannot-grow-on`, `empty-prompt`) |
| 401 | `authentication_error` | a key is set and the request does not carry it |
| 404 | `not_found_error` | no nurse here or on a trusted peer grows the model, or any other route |
| 413 | `request_too_large` | the body is past 64 MiB |
| 429 | `rate_limit_error` | the tenant's bucket is empty; `Retry-After: 60` |
| 502 | `api_error` | the nurse broke off |
| 503 | `api_error` | humd is away, or has not admitted the bee yet, or went away mid-bloom |
| 500 | `api_error` | the server failed |

A stream that fails before the bloom first grows fails with its status,
as a request without `stream` does. Once it has begun, it ends with an
`error` event of the type its status gives, and no `message_stop`. When
humd goes away mid-bloom, every open request hears that as the bloom
breaking off. A client that hangs up, streamed or not, cancels its bloom.

## Tenants, usage and limits

A request names its tenant in `x-tenant` (letters, digits, `_` and `-`;
anything else is dropped), or is the tenant `default`. Every other
tenant's sids begin with its name, `<tenant>:`, so tenants stay apart on
the wire. Each tenant has:

- a rate limit: a bucket of 60 requests, refilled at 1 a second, unless
  `rate` says otherwise;
- its usage, `{ inputTokens, outputTokens, totalTokens, requests }`, counted
  from each bloom's nectar (`input_tokens`, `output_tokens`), answered by
  `GET /v1/usage` as `{ tenant, usage }`, and kept in `usage.json` in the
  hive's state, written at most every 30 seconds. A turn that goes back
  with `tool_use` counts as a request that took nothing; the nectar comes
  with the turn that wilts.

`audit.log`, beside it, has one JSON line per `/v1/messages` request: when,
tenant, model, sid and whether it streamed. The tenant is the caller's
word: anyone with the key can name any tenant and read its usage.

## Configure

| env | config key | default | |
|---|---|---|---|
| `ANTHROPIC_SERVER_HOST` | `host` | `127.0.0.1` | where it listens |
| `ANTHROPIC_SERVER_PORT` | `port` | `14622` | |
| `ANTHROPIC_SERVER_API_KEY` | `apiKey` | none | the key every route requires, as `x-api-key` or `Authorization` (the key, or `Bearer <key>`); with none, the server is open |
| `ANTHROPIC_SERVER_RATE_CAPACITY` | `rate.capacity` | `60` | requests a tenant's bucket holds |
| `ANTHROPIC_SERVER_RATE_REFILL` | `rate.refillPerSec` | `1` | requests a second it refills |
| `HUM_THRUM_SOCK` | | humd's socket | where the bee's link finds humd |
| `HUM_CLI` | | `hum` on `PATH`, else `~/.local/bin/hum` | the `hum` the bee links through |

The config file is `$XDG_CONFIG_HOME/hum/hives/anthropic-server.json`
(`~/.config/hum/hives/anthropic-server.json` by default), and the
environment wins over it. The state is in
`$XDG_STATE_HOME/hum/hives/anthropic-server/`.

Installed, the server runs under orchd, and its Orchfile passes it none of
these variables; the config file is how to set its host, port, key and
rate.

```json
{ "port": 14622, "apiKey": "sk-mine", "rate": { "capacity": 120, "refillPerSec": 2 } }
```

## Run

```bash
hum hive anthropic-server install     # build it and keep it running
```

The server listens, then enters its bee
(`hum bee link --hive anthropic-server --as main --role forager`) and
prints `anthropic-server listening on ...` once humd admits it. A refusal
ends the process. To run it yourself, with its hive installed:

```bash
cd hives/anthropic-server
./build
node dist/index.js
```

It is the same bee as the one install runs, so run one or the other.

## Use

```bash
curl http://127.0.0.1:14622/v1/messages \
  -H "Content-Type: application/json" \
  -d '{
    "model": "claude-sonnet-4-6",
    "max_tokens": 1024,
    "messages": [{ "role": "user", "content": "ping" }]
  }'
```

```ts
import Anthropic from "@anthropic-ai/sdk";
const client = new Anthropic({ baseURL: "http://127.0.0.1:14622", apiKey: "anything" });

const message = await client.messages.create({
  model: "claude-sonnet-4-6",
  max_tokens: 1024,
  messages: [{ role: "user", content: "ping" }],
});

const stream = client.messages.stream({
  model: "claude-sonnet-4-6",
  max_tokens: 1024,
  messages: [{ role: "user", content: "ping" }],
});
for await (const event of stream) console.log(event);
```

## Test

```bash
pnpm install && pnpm run build
cargo build -p hum -p humd           # from the repo root
bun test
```

The end-to-end tests run the official Anthropic SDK against this server
under Node, a humd of their own built from this checkout, and a scripted
nurse that grows `claude-lab`. The unit tests read conversations and tools
as the server does.

## What is not yet

- **No `/v1/models` and no `/v1/messages/count_tokens`.**
- **One bee for every caller.** Every caller prompts as the forager
  `main`, so humd hears no caller apart from another, and a key admits a
  caller to every tenant.
- **A tool result carries only its words.** Its images and documents are
  not sent.
- **humd keeps every sigil a connection claims until the connection
  closes**, `cleanup` or not. The server holds one connection for its life
  and claims a sigil per conversation, so humd's memory grows a little
  with every conversation.

## See also

- [`openai-server`](../openai-server): OpenAI's API in front of hum, with
  a bee for each key.
- [`thrum-clients/ts`](../../thrum-clients/ts): the thrum client this bee
  uses.
- [THRUM.md](../../THRUM.md): the protocol on humd's side.

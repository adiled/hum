---
title: "ollama-server (Rust)"
description: "Ollama for hum, both ways: Ollama's chat and generate API in front of humd, and a nurse that grows blooms through Ollama"
---

# ollama-server (Rust)

> _Ollama for hum, both ways: Ollama's chat and generate API in front of humd, and a nurse that grows blooms through Ollama_

The hive has two bees, each its own binary. `ollama-server` is a
forager, entered under the label `main`: it serves Ollama's REST API
over HTTP on port `14624`, beside hum's other surfaces, and opens a
bloom for each `/api/chat` or `/api/generate` request, which humd hands
to whichever nurse grows the model the request names, here or on a
trusted peer. `ollama-nurse` is a nurse, entered under the label
`nurse`: it grows blooms for the models it claims by calling Ollama on
Ollama's own port, `11434`. The two run side by side with a stock
Ollama, which keeps its port.

The forager is built with [axum](https://github.com/tokio-rs/axum) on
`hum-thrum`, and the nurse on [`hum-nest`](../../hum-nest). The hive is
its own cargo workspace, outside the repo root's.

## Propensity

| bee | statefulness | richness | wire |
|---|---|---|---|
| `ollama-server` | convention-stateful | medium | `ollama/api` |
| `ollama-nurse` | `stateless_per_call` | none claimed | `ollama-server` |

The forager speaks `hello`, `prompt`, `cancel`, `cleanup`, `unfurl`,
`wilt`, `error`, `tendril-reach` and `tendril-result`. The nurse speaks
what every hum-nest nurse does: `hello`, `prompt`, `cancel`, `cleanup`,
`curate`, `tendril-result`, `unfurl`, `wilt`, `error`, `tendril-reach`,
`tendril-info` and `pulse`.

## The forager

```
client                              ollama-server                     humd
  │                                       │                            │
  │  POST /api/chat                       │                            │
  ├──────────────────────────────────────►│                            │
  │   { model, messages, stream,          │                            │
  │     tools? }                          │  chi:"prompt"              │
  │                                       ├───────────────────────────►│
  │                                       │  chi:"unfurl"              │
  │                                       │◄───────────────────────────┤
  │  {"message":{"content":"Hi"},"done":false}                         │
  │◄──────────────────────────────────────┤                            │
  │                                       │  chi:"wilt"                │
  │                                       │◄───────────────────────────┤
  │  {"done":true,"done_reason":"stop",...}                            │
  │◄──────────────────────────────────────┤                            │
```

Ollama's streaming response is already line-delimited JSON, one line per
unfurl. A request streams NDJSON (`application/x-ndjson`) when its
`stream` is true or absent, and gets one JSON object otherwise. A stream
holds its headers until its bloom has something to say.

Every request shares one link to humd. It opens on the first request,
which waits up to ten seconds for humd to admit the bee and breathe what
it can reach, and closes once it has gone unused for ten minutes with no
bloom to follow and no tool call held, to open again on the next
request.

### Endpoints

| route | what |
|---|---|
| `POST /api/chat` | a conversation; NDJSON streaming |
| `POST /api/generate` | a single prompt, with an optional `system`; NDJSON streaming |
| `GET /api/tags` | the models the forager's breath says nurses grow, here and on trusted peers, or the configured list (below) |
| `GET /` | health probe, `Ollama is running`, as the real Ollama answers |

No other Ollama route is served: `/api/show`, `/api/version`,
`/api/embed`, `/api/pull`, `/api/ps` and the OpenAI-compatible `/v1`
routes are 404.

### What flows where

| Ollama | thrum |
|---|---|
| a `/api/chat` request | `chi:"prompt"` on its conversation's sigil (below) |
| a `/api/generate` request | `chi:"prompt"` on a fresh sid, cleaned up once its bloom closes |
| `model` | `prompt.modelId`, whatever the model; humd matches it exactly against the ids nurses claim |
| the last `user` message's `content` and `images` | `prompt.content`: a text petal, and a pollen petal for each image |
| the messages before it | `prompt.graft`, a bloom for each user turn (below) |
| `messages[]` with `role: "system"`, joined, or `/api/generate`'s `system` | `prompt.systemPrompt` |
| `/api/generate`'s `prompt` and `images` | `prompt.content` |
| `tools[]` of `type: "function"` | `prompt.tools[]` (`name`, `description`, `inputSchema`, an empty object schema when the tool gives none) |
| streamed `message.content`, or `response` | `chi:"unfurl"` of a text petal |
| a `message.tool_calls` line, each call's `id` its `callId` | `chi:"tendril-reach"` for one of the request's tools |
| a follow-up request that ends in `tool` messages | `chi:"tendril-result"` for each held reach, its `output` the message's `content` |
| the `done: true` line, its `done_reason` | `chi:"wilt"`, its `reason`; on `/api/chat`, its nectar too (`input_tokens`, `output_tokens`) |
| an error status, or an `{"error": ..., "done": true}` line once a stream has begun | `chi:"error"`, for example humd's `no-nurse` |
| a client that goes away mid-bloom | `chi:"cancel"` |

The forager's hello claims no `builds`, and its prompts carry no `cwd`,
so its blooms build nowhere.

### A conversation

Ollama's chat API sends the whole conversation every time. The forager
reads it as blooms, one for each user turn: its words and images are the
bloom's content, and what grew in it is the assistant's `thinking` as a
sepal, its `content` as text, its `tool_calls` as tendrils, and each
`tool` message as the grasp of a call. A `tool` message answers the call
its `tool_call_id` names, else the first unanswered call of its
`tool_name`, else the first unanswered. The last user turn opens the
prompt, and the blooms before it are its graft, so a nurse that did not
grow them grows from them. A conversation that ends with the assistant's
words or with tool answers asks for a bloom to grow on: the prompt has
no content, and every bloom is its graft.

A conversation keeps one sigil while it only grows. The forager
remembers each conversation it answered by a trace of its system prompt,
the words of every turn and each tool call's name, arguments and answer,
and a request whose earlier turns match one continues that sigil. One no
one has seen, one whose history was edited, and one already growing get
a sigil of their own. The forager remembers 4096 such traces, and cleans
up a sigil no trace leads to any more.

### Tool calls

A reach for a tool the request listed streams as a `tool_calls` line, in
Ollama's shape, `{"id", "function": {"index", "name", "arguments"}}`.
Once reaches stop coming for 150 ms, the turn goes back to the client,
`done_reason: "stop"`, as Ollama's does, and the bloom waits on its
reaches. The client runs the tools and sends the conversation again,
ending in its `tool` messages; the forager answers each held reach with
a `tendril-result` and streams the bloom on as that request's response.
A call comes back by its `id`, or, from a client that drops ids, by its
name and arguments. Answers it cannot match to reaches of one bloom it
holds go to a new prompt instead, whose graft carries them, as does a
follow-up for a bloom still growing. A reach is held for thirty minutes,
and a humd that goes away takes the blooms it held reaches for with it.
A reach that comes after its turn went back to the client is answered
at once with an error asking the cell to call again.

### Errors

A bloom that breaks off before the response starts answers with a
status and `{"error": ...}`, as Ollama's own does: `404` when no nurse
grows the model (`no-nurse`), `400` for a prompt the nurse cannot grow
(`cannot-grow-on`, `empty-prompt`), `503` when humd went away or the
peer growing the bloom left (`humd-away`, `nurse-gone`), which a client
may try again, and `502` for any other error. Once a stream has begun,
an error is its last line, `{"error": ..., "done": true}`. When humd
refuses the forager's bee, a request gets `502` with the reason, and
when humd does not admit it within ten seconds, `503`.

A client that goes away mid-bloom, a stream it stops reading or a
request it drops, cancels the bloom.

### Configure

| env | default | what |
|---|---|---|
| `OLLAMA_SERVER_PORT` | `14624` | HTTP listen port |
| `OLLAMA_SERVER_HOST` | `127.0.0.1` | HTTP listen host |
| `OLLAMA_SERVER_MODELS` | the breath's models | comma-separated list `/api/tags` returns in their place |
| `OLLAMA_SERVER_LABEL` | `main` | the label the forager's bee is entered under |
| `HUM_THRUM_SOCK` | the socket humd listens on, `$XDG_STATE_HOME/hum/thrum.sock` by default | humd's thrum socket, for both bees |

An empty `OLLAMA_SERVER_*` variable is no variable. The host, port and
models live in the
hive's config file too, `~/.config/hum/hives/ollama-server.json`, beside
the nurse's `nest`:

```json
{
  "host": "127.0.0.1",
  "port": 14624,
  "models": ["claude-sonnet-4-6"]
}
```

The environment wins over the file, and the file over the defaults. The
Orchfile declares an empty `ARG` for each `OLLAMA_*` variable of both
bees and hands it to its unit, so `OLLAMA_SERVER_PORT=11500 hum hive
ollama-server install` sets it for the installed forager, and an unset
one keeps the bee's own default.

## The nurse

`ollama-nurse` claims the model ids in `OLLAMA_NURSE_MODELS` and grows
each bloom by calling Ollama at `OLLAMA_NURSE_URL`. It asks Ollama
nothing up front: humd routes a prompt for one of those ids here, and
the nurse passes the id to Ollama as it stands, so pull each model into
Ollama first (`ollama pull smollm:1.7b`). A prompt that names no model
gets the first id in the list.

| env | default | what |
|---|---|---|
| `OLLAMA_NURSE_URL` | `http://127.0.0.1:11434` | Ollama's API, on Ollama's own port |
| `OLLAMA_NURSE_MODEL` | `smollm:1.7b` | the model when the list is empty, and the list's default |
| `OLLAMA_NURSE_MODELS` | `OLLAMA_NURSE_MODEL` | the model ids it claims, comma-separated |
| `OLLAMA_NURSE_CTX` | `8192` | Ollama's `num_ctx` for every bloom |

An empty `OLLAMA_NURSE_*` variable is no variable too.

A bloom grows through Ollama's `/api/chat`:

- **Messages.** The system prompt, then each grafted bloom (its words
  and images as a `user` message, what grew as `assistant` messages, its
  tendrils as `tool_calls` and its grasps as `tool` messages), then the
  prompt's own petals. An image rides on its message when its grain is
  inline data.
- **How it grows.** `temperature`, `prune` (`top_p`, `top_k`), `seed`,
  `frost` (`stop`) and `season` (`num_predict`) become Ollama's
  options; `trellis` (`"json"` or a schema) becomes `format`; `sepal`
  becomes `think` (`false` for effort `none`, the effort itself for
  `low`, `medium` or `high`, else `true`).
- **The reply.** Its words unfurl as a text petal and its thinking as a
  sepal, as they stream. The bloom wilts `stop`, or `max_tokens` when
  Ollama stopped for length, with Ollama's token counts as nectar. A
  `cancel` wilts it `cancel`.
- **Growing on.** A prompt with no words whose graft is plain grown text
  goes to `/api/generate` instead, as that text raw, or with the prompt's
  `scion` as its `suffix`. A graft that holds a conversation cannot grow
  on that way, and is refused with `cannot-grow-on`; a prompt with
  neither words nor a graft, with `empty-prompt`.
- **Errors.** `ollama_connection_error` when Ollama cannot be reached,
  `ollama_api_error` when it answers with an error,
  `ollama_stream_ended` when its reply ends before it is done, and
  `ollama_cell_stilled` for a prompt its cell was stilled before it grew.

The nurse hands Ollama no tools, so its blooms reach no tendrils.

Its nest's limits are in the `nest` section of the same config file. A
cell runs no process: it lives for its sigil, growing each prompt the
nest hands it there in turn, each from that prompt's own graft, until
the nest ends it. `maxCells` (default 8) is how many sigils' cells it
keeps at once, and the headroom the nurse claims; it pulses
`cell-spawned` as each cell is raised. Past `maxCells`, a new sigil's
prompt evicts the least recently used cell, `idleMs` (default five
minutes) ends a cell that has grown nothing for that long, and
`bounds.wallClockMs`, when set, one that has lived that long since it
was raised; each pulses `cell-evicted`. A `cancel` ends its cell too,
which pulses `cell-died`. However a cell ends, the bloom growing in it
wilts `cancel`, and a prompt still waiting for it is answered
`ollama_cell_stilled`. The other `bounds` cap nothing here.

The forager listens on `14624` and the nurse calls Ollama on `11434`,
so neither stands in the other's way. The nurse marks every call it
makes, and the forager answers a marked call with `508 Loop Detected`,
so a nurse whose `OLLAMA_NURSE_URL` points at its own forager fails its
blooms with `ollama_api_error` rather than looping them back into hum.

## Run

```bash
hum hive ollama-server install
```

`install` builds both binaries (`cargo build --release --locked`) and
keeps both of the Orchfile's units running under orchd, `ollama-server`
and `ollama-nurse`, in the `hum-ollama-server` namespace. A setting
rides in the environment of the install:

```bash
OLLAMA_NURSE_MODELS=llama3.2,qwen3:4b hum hive ollama-server install
```

Either binary runs from its directory too, once the hive is installed on
the humd it lands on (each enters its own bee when it starts):

```bash
cd hives/ollama-server
cargo run --release --bin ollama-server
OLLAMA_NURSE_MODELS=llama3.2 cargo run --release --bin ollama-nurse
```

## Test

```bash
cd hives/ollama-server && cargo test
```

The forager's tests run it as orchd would, against a humd in a root of
its own and a nurse that answers by what it is asked: the models it
lists, a conversation carried as the graft and kept on one sigil, a tool
call answered in the next request with ids and without, a bloom that
breaks off, a client that goes away, and the one link every request
shares. The nurse's run its cells against a stand-in Ollama: a cell
grows every prompt its nest feeds it, and one stilled or past its wall
clock wilts its bloom. The unit tests cover the conversation's reading,
its traces, the configuration, the forager refusing the nurse's marked
call, and the nurse's mapping of a graft, its options and growing on.

Against a real Ollama, with a model pulled into it:

```bash
OLLAMA_LIVE_MODEL=llama3.2 cargo test --test live -- --ignored
```

## Use

Any Ollama client, pointed at the forager's port, with a model
`/api/tags` lists. Plain curl:

```bash
curl http://localhost:14624/api/chat \
  -H "Content-Type: application/json" \
  -d '{
    "model": "claude-sonnet-4-6",
    "stream": true,
    "messages": [{ "role": "user", "content": "ping" }]
  }'
```

Ollama JS client:

```ts
import { Ollama } from "ollama";

const ollama = new Ollama({ host: "http://localhost:14624" });
const r = await ollama.chat({
  model: "claude-sonnet-4-6",
  messages: [{ role: "user", content: "ping" }],
  stream: true,
});
for await (const chunk of r) process.stdout.write(chunk.message.content);
```

LangChain:

```python
from langchain_ollama import ChatOllama
llm = ChatOllama(model="claude-sonnet-4-6", base_url="http://localhost:14624")
print(llm.invoke("ping"))
```

## What is not yet

- **Ollama's own counts are not there.** The final line's `done_reason`
  is the wilt's (`stop`, `max_tokens`, …), not Ollama's `length`; an
  `/api/chat` line carries `input_tokens` and `output_tokens` in place
  of `prompt_eval_count` and `eval_count`, and an `/api/generate` line
  carries no counts.
- **Thinking is not shown.** A sepal the bloom unfurls does not reach
  the client as `message.thinking`.
- **Request fields not read:** `options`, `format`, `keep_alive`,
  `think`, and on `/api/generate`, `raw`, `suffix` and `context`.

## See also

- [`openai-server`](../openai-server), [`anthropic-server`](../anthropic-server): sibling surfaces.
- [`hum-nest`](../../hum-nest): the nest the nurse runs on.
- [hives](../README.md): how a hive is installed and how its bees enter.
- [THRUM.md](../../THRUM.md): the protocol on humd's side.
- [adiled.github.io/hum](https://adiled.github.io/hum/): docs site.

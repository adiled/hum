---
title: "openai-server"
description: "OpenAI's HTTP API in front of hum, and bees behind it that call an OpenAI-compatible API"
---

# openai-server

> _OpenAI's HTTP API in front of hum, and bees behind it that call an OpenAI-compatible API._

Any client that speaks OpenAI drives hum through this hive without
knowing hum exists: the official SDKs, coding agents, chat UIs, editor
completion. Its requests are grown by whatever nurse humd can reach, and
its embeddings distilled by whatever attendant distils by the model they
name: on this humd, or on a peer whose bee's hive is installed here, when
this humd trusts remote nurses or attendants (`humd.trustRemoteNurses`,
`humd.trustRemoteAttendants` in `hum.json`). The hive also serves the API
at `OPENAI_API_BASE` to hum, so that API's models are grown and distilled
for every other forager too.

## Its bees

| bee | role | does |
|---|---|---|
| `key-<name>`, one per key | forager | asks humd for each request its key's callers make; `key-default` for `apiKey` |
| `anonymous` | forager | the same, when no key exists |
| `main` | nurse | grows the upstream's models |
| `scent` | attendant | distils the upstream's embedding models |

`main` and `scent` enter themselves when the process starts and follow
the upstream's model list, read at start and every five minutes (every
15 seconds until it first answers). A model is `scent`'s when the
config's `scents` lists it, or, with no such list, when its id says
`embed`; every other is `main`'s. A request upstream is tried again on a
network failure, a 429 or a 5xx, up to three times. If `main` or `scent`
is refused, or its link ends, the process ends, for orchd to start it
again.

## Propensity

| bee | statefulness | richness | wire |
|---|---|---|---|
| `key-<name>`, `anonymous` | `stateful` | `rich` | `openai/api` |
| `main` | `stateless_per_call` | none | `openai/chat-completions` |
| `scent` | `stateless` | `lean` | `openai/embeddings` |

A forager keeps what its callers' conversations need between requests:
the sigil each grows on, the tool calls it holds and the responses it
made. It carries a conversation's earlier turns whole, as its prompt's
graft. Each forager and `main` list the chis `hello`, `prompt`,
`cancel`, `cleanup`, `unfurl`, `wilt`, `error`, `tendril-reach`,
`tendril-result` and `tendril-info` in their hellos, and `scent` lists
`hello`, `tendril-reach` and `tendril-result`, so humd sends each only
those among a bloom's tones. `main`'s hello also names where the surface
listens (`bind`), for the ensemble's manifest of it.

## The surface

| route | |
|---|---|
| `POST /v1/chat/completions` | streamed or not, with tools, images, audio and files; thinking comes as `reasoning_content` |
| `POST /v1/responses`, `GET` and `DELETE /v1/responses/{id}` | the Responses API, `previous_response_id` and `conversation` included |
| `POST /v1/completions` | text grown on from as it stands, `suffix` for filling in the middle, several prompts at once |
| `POST /v1/embeddings` | floats or base64, `dimensions` passed on |
| `GET /v1/models`, `/v1/models/{id}` | what the caller's bee can reach, each `owned_by` the hive that serves it; a model that grows words lists, as `tools`, the tools attendants serve |
| `GET /v1/usage` | what the calling key has taken: `{ "key", "usage": { "requests", "promptTokens", "completionTokens", "totalTokens" } }` |
| `GET /` | `hum-openai-server`, with no key, for a health check |

Every route answers without its `/v1` too. A request for words may leave
`model` out, and any nurse with room grows it.

**Conversations.** OpenAI's APIs send the whole conversation every time;
hum keeps it in a sigil. A request's earlier turns travel as the
prompt's `graft`, so any nurse can grow it, and its last turn as the
prompt. A conversation that ends with tool answers or with the
assistant's own words asks for a bloom that grows on: the prompt has no
words, and the whole conversation is its graft. A conversation stays on
one sigil while it grows, so a nurse that keeps sessions picks up where
it was. A client may name its line with an `X-Hum-Sid` header, kept
apart from every other caller's. Otherwise the sigil is found by the
newest tool call or answer id the conversation holds that this caller
was given, or, with no tools, by the words of its last two turns. The
whole history is not compared: clients compact it all the time, dropping
old tool pairs, trimming, sending a turn back as one message or without
the words said between its calls, while a call's id comes back as it was
given. A caller that edits an older turn keeps its sigil, but a sigil
goes on only from where it last left off, so a copy of the conversation
taken before then gets one of its own; a conversation already growing
gets one of its own, and callers on different keys never share one. A
conversation that ends in tool answers is also found by the turns before
them, so an answer that came after its call was let go, or a call and
answer a caller made up to hand the model news, grows on in the session
that came before.

**Settings.** `temperature`, `top_p`, the token cap, `stop`, `seed`, the
JSON shape (`response_format`, `text.format`), `tool_choice`,
`parallel_tool_calls` and the reasoning effort become the prompt's, for
whichever nurse grows it. Penalties and `logit_bias` ride under this
hive's own `ext`, which only `main` reads. `n` or `best_of` above 1,
`logprobs`, and token arrays for `prompt` or `input` are refused with a
400.

**Tools.** A caller's function tools are tools a bloom may reach back to
it for, by their exact names, in the chat shape or the Responses one; a
tool of any other type, one only OpenAI runs, is left out. Calls come
back as `tool_calls`, or `function_call` items, several at once when the
model makes several, and the caller's answers, on the same key, carry the
bloom on. The server holds each call for ten minutes, less than the nest
waits for it, and keeps the key's link open while it holds one. A call
it lets go unanswered, as the ten minutes pass or as the key's link
ends, cancels the bloom that
reached for it, unless a prompt has since opened another bloom on the
same sigil; a link that ends on its own leaves the cancel to the key's
next one. An answer that comes after the server has let its call go,
past those ten minutes or across a restart, opens a bloom that grows
on from the whole conversation, which a nurse that cannot grow on from a
graft refuses with a 400.

A tool an attendant serves runs there, and an attendant on this humd
takes a name even from the caller's own tool, so `GET /v1/models` lists
these under each model's `tools` for a client to keep its own names
apart. A response shows such a call as an `mcp_call` item, output and
all. Chat completions has no place for it, so it rides on the assistant
message as `mcp_calls`, each shaped as that item, streamed as a delta
like `tool_calls`; a caller that sends the message back sends them too,
and they are read as what grew. The hive's prompts name no `cwd` and its foragers do not build, so
its blooms build nowhere: a reach for `bash` comes back unserved, unless
the caller offered a tool named `bash`, which reaches the caller like any
other.

**Responses.** A response is kept for the key that made it, unless
`store` is false: `GET` reads it back, `DELETE` forgets it, and
`previous_response_id` grows on from it, on its sigil. `conversation`
names a thread by any id the caller picks, and each response in it
follows the thread's latest; there is no `/v1/conversations` here.
Responses are kept in memory, the latest 2048, so none outlives a
restart. A thread is kept only while its latest response is: once that
response is deleted or among those forgotten, the thread's next response
starts it over. `background` is refused.

**Completions.** Each prompt is a bloom of its own, carried as a graft of
plain text with no words of its own, so the nurse grows on from where the
text stops, and `suffix` is the text it grows to meet. Only a nurse that
grows on from a graft serves it, and any other answers 400; `main` does,
through the upstream's `/completions`. `max_tokens` is 16 unless said,
and `echo` puts the prompt before what grew.

**Failures.** Each comes in OpenAI's error shape. A missing or wrong key
is a 401, a key past its rate a 429, a body over 64 MiB a 413, an unknown
model a 404, a request this server or the nurse cannot grow a 400, a
nurse or upstream failure a 502, humd away or refusing the caller's bee a
503, and a request past its time a 504. A stream holds its status until
it first sends something, a word or the SSE comment that keeps a quiet
stream open every 15 seconds, so a failure before then is that status;
after, the stream ends with its error as its last event, which in the
Responses API, whose stream begins at once, is `response.failed`. A
caller that hangs up cancels its bloom.

## Configure

| env | config key | default | |
|---|---|---|---|
| `OPENAI_SERVER_HOST` | `host` | `127.0.0.1` | where the surface listens |
| `OPENAI_SERVER_PORT` | `port` | `14620` | |
| `OPENAI_SERVER_API_KEY` | `apiKey` | | one more key, named `default` |
| `OPENAI_SERVER_TIMEOUT_MS` | `timeoutMs` | 15 minutes | how long one request may take |
| | `cors` | none | origins a browser may call from, or `"*"` |
| | `rate` | none | `{ "capacity": 60, "refillPerSec": 1 }`: a bucket for each key |
| | `scents` | ids that say `embed` | the upstream models `scent` distils by |
| `OPENAI_API_BASE` | | `https://api.openai.com/v1` | the API `main` and `scent` call |
| `OPENAI_API_KEY` | | | its key |

The config file is `$XDG_CONFIG_HOME/hum/hives/openai-server.json`
(`~/.config/hum/hives/openai-server.json` by default), and an environment
variable wins over it. The hive's state, its keys and their usage, is in
`$XDG_STATE_HOME/hum/hives/openai-server/`
(`~/.local/state/hum/hives/openai-server/` by default).

## Run

```bash
OPENAI_API_KEY=sk-... hum hive openai-server install                     # build it, keep it running
OPENAI_API_BASE=http://127.0.0.1:8000/v1 hum hive openai-server install  # any OpenAI-compatible API
```

`install` builds the hive with pnpm (npm without it) and runs
`node dist/index.js` under orchd, started again whenever it ends. Of the
environment, it passes on only `OPENAI_API_BASE` and `OPENAI_API_KEY`, as
they are when it runs, so install again to change them, and set the rest
in the config file. With no upstream answering `/models` (api.openai.com
without a key, say), `main` and `scent` serve nothing, and the surface
still serves every nurse and attendant humd reaches.

`node dist/index.js` runs the server in the foreground, on a humd where
the hive is installed, with `hum` on its `PATH` (or at `HUM_CLI`).

## Keys

```bash
node dist/index.js key add alice     # prints alice's token, once
node dist/index.js key list          # each key's name, and when it was made
node dist/index.js key remove alice
```

Run them from the hive's directory. A name is letters, digits, `-`, `_`
and `.` (not first), at most 64 of them, and neither `anonymous` nor
`default`, which is `apiKey`'s, so the two never meet. A token
(`sk-hum-...`) is kept only as its sha256, in `secrets/keys.json` under
the hive's state, and the server reads the file again when it changes, so
a key counts as soon as it is added and stops as soon as it is removed. A
caller bears its token as `Authorization: Bearer <token>`, or as an
`api-key` header. With no key at all, anyone who reaches the port may
call, as the one bee `anonymous`.

Each key is one forager bee, so humd hears every caller apart. A key's
bee is entered and nestles on the key's first request. Its link closes
once ten minutes pass with no request on the key while it grows no bloom
and holds no tool call, and opens again on the next request. Removing the
key is what shuts its caller out: `hum bee <id> exit` has humd refuse
only the bee's next hello, and the link enters the bee again each time it
opens.

Every request counts against its key, with the tokens its bloom took in
and gave out. The counts are kept in `usage.json` under the hive's
state, written every 30 seconds and when the process stops.

## Use

```ts
import OpenAI from "openai";
const client = new OpenAI({ baseURL: "http://127.0.0.1:14620/v1", apiKey: "sk-hum-..." });
const r = await client.chat.completions.create({
  model: "claude-opus-4-7",                       // any id GET /v1/models lists
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
under Node, a humd of their own built from this checkout, a scripted
upstream, and an attendant serving one tool; `test/holds.test.ts` adds a
nurse that grows nothing and hears every cancel. `test/claude.live.test.ts`
runs only with `HUM_LIVE_CLAUDE=1`: real claude behind this server,
through the claude-cli nurse (build it first with `cargo build` in
`hives/claude-cli`), signed in and spending as the person's own Claude
Code. `HUM_LIVE_MODEL` picks the model, `claude-haiku-4-5` by default.

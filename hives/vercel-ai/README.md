---
title: "vercel-ai"
description: "a Vercel AI SDK provider: a LanguageModelV3 whose every call is a bloom on hum"
---

# vercel-ai

> _a Vercel AI SDK provider: a `LanguageModelV3` whose every call is a bloom on hum_

`@hum/vercel-ai` is a library, not a server. Code written against the AI
SDK (`generateText`, `streamText`) gets a model whose calls prompt humd,
and humd hands each bloom to whichever nurse grows the model, on this
humd or on a trusted peer. It implements `LanguageModelV3` from
`@ai-sdk/provider` 3, the provider interface of `ai` 6.

## Its bee

The provider is a forager bee of the hive `vercel-ai`, living in the
process that imports it. The first call under a label starts its bee:
the thrum client runs `hum bee link --hive vercel-ai --as <label> --role
forager`, which enters it (its key at `$XDG_STATE_HOME/hum/bees/<id>.key`),
signs its hello and holds its connection to humd. The label is `main`
unless `createHum` is given another, so two apps on one humd that each
name their own are two bees. Two processes under one label are one bee,
and the newer connection replaces the older.

- The hive must be installed on the humd. If it is not, or humd refuses
  the bee, the call fails with the refusal, and the next call tries again.
- A call waits up to 10 seconds for humd to admit the bee, then fails
  with `humd is away`. When humd goes away mid-bloom, every open stream
  gets an `error` part and ends.
- The link closes once nothing has been open for a second, so a script
  that is done exits on its own, and the next call opens it again.
  `close()` closes it at once, and cancels every bloom still open or
  held.
- The client finds `hum` at `HUM_CLI`, else on `PATH`, else at
  `~/.local/bin/hum`, and the link finds humd at `HUM_THRUM_SOCK`, else
  where humd says it listens.

Its hello carries its `version` and `source`, its `propensity`
(`convention-stateful`, `rich`, wire `ai-sdk/language-model-v3`) and its
`chis`: `hello`; the `prompt`, `cancel`, `cleanup`, `attach` and
`tendril-result` it sounds; and the `unfurl`, `wilt`, `error`,
`tendril-reach` and `tendril-info` it hears. humd fans it no other tone
of a bloom.

## Install

```bash
hum hive vercel-ai install                     # teach humd the kind, and build dist/
npm install ai /path/to/hum/hives/vercel-ai    # in your project
```

Installing the hive runs its `./build` and nothing else: the package is
built in place, a project depends on the directory, and no service runs,
since the hive's bees live in the apps that import it.

## Use

```ts
import { createHum } from "@hum/vercel-ai";
import { generateText, stepCountIs, streamText, tool } from "ai";
import { z } from "zod";

const hum = createHum();

const { text } = await generateText({
  model: hum("claude-sonnet-4-6"),
  prompt: "explain monorepos in 3 sentences",
});

const weather = tool({
  description: "The weather in a city.",
  inputSchema: z.object({ city: z.string() }),
  execute: async ({ city }) => `sunny in ${city}`,
});
const { textStream } = streamText({
  model: hum("claude-sonnet-4-6"),
  tools: { weather },
  stopWhen: stepCountIs(5),
  prompt: "what is the weather in Paris?",
});
for await (const chunk of textStream) process.stdout.write(chunk);

await hum.close();
```

`createHum(config)` returns a function from a model id to a `HumModel`,
also callable as `.languageModel(id)`, and `.close()` closes its bee;
`HumModel` is exported too. The model id is the prompt's `modelId`, so it
must be one a nurse humd can reach grows.

It takes two settings:

- `cwd` is where the bloom builds: a `hum://<humdid>/<path>` address, or
  a path on this host, taken as `hum:///<absolute path>`, this humd.
  Without it the bloom builds nowhere, and its `bash` reaches are
  answered as unserved.
- `label` is the bee's label, `main` when left out.

```ts
const hum = createHum({ cwd: process.cwd(), label: "my-app" });
```

## The conversation

The AI SDK sends the whole conversation on every call, and the provider
reads it as blooms. Each user message opens one, its text and files its
content, and what the assistant said, reasoned and called, and what its
tools gave back, are what grew in it. The last user message is the
prompt's `content`, and the blooms before it are its `graft`. A
conversation that ends with tool results, or with the assistant's own
words, sends no content, and every bloom is the graft, for the nurse to
grow on from.

A conversation keeps one sigil while it only grows: a call whose earlier
blooms are the ones this bee last grew on a sigil prompts on that sigil
again, so a nurse whose cell grew them grows on from its own. A
conversation the bee has not seen, one whose history was edited, or one
already growing gets a sigil of its own, `vai-<uuid>`. The match reads
the system prompt, the words of each turn and the ids of each tool call.

## Tools

A function tool goes on the prompt as `{ name, description, inputSchema }`,
its schema an empty object when it gives none. When the bloom reaches
one, humd routes the `tendril-reach` back to this bee, and the stream
carries it as a `tool-call`. Once reaches have stopped coming for 150 ms,
the stream finishes with reason `tool-calls`, and the bee holds each
reach, for up to ten minutes, while the bloom waits.

The AI SDK runs the tools and calls again, with their results at the end
of the prompt. Results that answer reaches the bee holds go back as
`tendril-result`s on the bloom's own sigil, and the new stream follows
that bloom as it grows on, with no new prompt. When the link that heard
the reaches has since closed, the bee `attach`es the sigil first. Results
that answer no held reach, after a restart or a hold that ran out, open a
prompt with the whole conversation as its graft, and the nurse grows on
from it.

So the SDK's tool loop (`stopWhen`) grows as one bloom. A call that stops
at its tool calls, as `generateText` and `streamText` do after one step
by default, leaves its bloom waiting until its nurse gives up on the
reach (ten and a half minutes on a nurse built on hum-nest), or until
`close()` cancels it.

A reach on a sigil whose stream has already finished is answered at once
with an error asking the cell to call again, and a reach for a tool the
call did not offer is answered as unserved.

Tools attendants serve, and `bash` where the bloom builds, are reached
inside humd. A nurse that tells its forager of each with `tendril-info`,
as one built on hum-nest does, shows it in the stream as a
provider-executed, dynamic `tool-call` with its `tool-result`, which the
SDK keeps and sends back with the conversation.

## What flows where

| AI SDK | thrum |
|---|---|
| a `doStream` or `doGenerate` call | `chi:"prompt"` on the conversation's sid, or `chi:"tendril-result"`s on a held one |
| the system messages | `prompt.systemPrompt`, joined |
| the last user message: text parts, file parts | `prompt.content`: text petals, pollen petals (image, audio or file grains, inline or by URL) |
| earlier messages: text, reasoning, files, tool calls, tool results | `prompt.graft`: text, sepal, pollen, tendril and grasp petals |
| `temperature` | `prompt.temperature` |
| `topP`, `topK` | `prompt.prune` (`p`, `k`) |
| `maxOutputTokens` | `prompt.season` |
| `stopSequences` | `prompt.frost` |
| `seed` | `prompt.seed` |
| `responseFormat` of type `json` | `prompt.trellis`: its schema, or `"json"` |
| `toolChoice` | `prompt.tendrils.reach`: `auto`, `none`, `required` or the tool's name |
| function `tools` | `prompt.tools`, each `{ name, description, inputSchema }` |
| a tool's result | `tendril-result.output`: its text, or its JSON as text; `isError` for an error or a denial |
| `abortSignal`, or cancelling the stream | `chi:"cancel"` |
| `text-start`, `text-delta`, `text-end` | `chi:"unfurl"` of a text petal |
| `reasoning-start`, `reasoning-delta`, `reasoning-end` | `chi:"unfurl"` of a sepal petal |
| `tool-input-start`, `tool-input-delta`, `tool-input-end` | `chi:"unfurl"` of a tendril petal, its `sap` the input |
| `tool-call` | `chi:"tendril-reach"` for a tool the call offered |
| a provider-executed `tool-call` and its `tool-result` | `chi:"tendril-info"` for a tendril served inside humd |
| `finish`, its reason and usage | `chi:"wilt"`, its `reason` and `nectar`; or reaches gone quiet, as `tool-calls` |
| `error`, then `finish` with reason `error` | `chi:"error"` |

The finish reason is `stop` for a wilt's `stop`, `end_turn` or `success`;
`length` for `length` or `max_tokens`; `tool-calls` for `tool_use` or
`tool_calls`; `content-filter` for `content_filter`; `other` for anything
else, and for an aborted call. Usage reads the nectar's `input_tokens`,
`cache_read_input_tokens`, `cache_creation_input_tokens` and
`output_tokens`. A stream that finishes at its tool calls has no nectar
yet: the bloom's comes with the call that sees it wilt.

`doGenerate` reads the same stream and keeps its content in order: text,
reasoning, tool calls, and the provider-executed calls with their
results. A bloom that breaks off makes it throw, with the error's
message.

Nothing else is sent: provider-defined tools, tool approval responses,
`presencePenalty`, `frequencyPenalty`, `headers` and `providerOptions`
are left out, and the call's `warnings` are always empty.

## What is not yet

- **An idle link is closed, not let go.** The thrum client has no way to
  let an open link stop keeping Node alive, so the bee closes its link a
  second after its last call, and the next call opens a new one.
- **Only text, sepal and tendril petals show.** A nurse's thorn, waggle,
  pollen, brix and bouquet petals, and a sepal's `propolis`, are left
  out of the stream.

## Test

```bash
pnpm install
cargo build -p hum -p humd           # from the repo root
pnpm test                            # builds dist/, then bun test
```

The end-to-end tests run the real `ai` package against the built
provider, a humd of their own built from this checkout, and a scripted
nurse and attendant (`test/lab.ts`); `test/done.mjs` is a script under
Node that asks once and must then exit. `test/conversation.test.ts`
reads prompts as blooms.

## See also

- [`thrum-clients/ts`](../../thrum-clients/ts): the thrum client the
  provider uses.
- [`openai-server`](../openai-server): an HTTP surface in front of hum,
  instead of an SDK provider.
- [THRUM.md](../../THRUM.md): the protocol on humd's side.

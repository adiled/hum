---
title: "vercel-ai"
description: "Vercel AI SDK provider for hum — pure, no OC coupling"
---

# vercel-ai

> _Vercel AI SDK provider for hum — pure, no OC coupling_

A `LanguageModelV3` provider that lets the Vercel AI SDK drive a hum
daemon. Code written against `ai` / `@ai-sdk/*` can call hum models
without learning thrum.

## Propensity

| statefulness | richness | wire shape | hides |
|---|---|---|---|
| stateless per-call | lean | Vercel AI SDK `LanguageModelV3` | pulse, breath, drone, perf-mark, tendril, permission-ask, tool-meta, session-ready, log, echo |

Each call mints a fresh sid; history flows only through what the
caller re-supplies. No persistent server-side state.

## Install

```bash
# Once published to npm:
npm install @hum/vercel-ai ai

# Today (workspace / git):
# (in a sibling repo, depend on this dir or vendor it)
```

## Use

```ts
import { humProvider } from "@hum/vercel-ai";
import { generateText } from "ai";

const hum = humProvider({
  // socket path; defaults to $XDG_RUNTIME_DIR/hum/hum.sock.thrum
});

const { text } = await generateText({
  model: hum("sonnet"),
  prompt: "explain monorepos in 3 sentences",
});
```

Streaming:

```ts
import { streamText } from "ai";

const { textStream } = await streamText({
  model: hum("sonnet"),
  prompt: "hello",
});
for await (const chunk of textStream) process.stdout.write(chunk);
```

## What flows where

| Vercel AI surface | hum chi |
|---|---|
| `generate` / `stream` request | `chi:"prompt"` |
| streamed text part | `chi:"chunk"` (text) |
| streamed tool-call part | `chi:"tool-call"` |
| finish | `chi:"finish"` |
| abort | `chi:"cancel"` |
| tool result feedback | `chi:"tool-result"` |

Everything else thrum carries (pulses, performance marks, drone
events, permission asks) gets dropped at the seam — the SDK has no
place to surface them.

## Status

Reference implementation. Provider surface tracks `@ai-sdk/provider`
v3+. Older SDK versions need an adapter shim.

## See also

- [`thrum`](../../thrum) — the npm package this bee imports.
- [`openai-server`](../openai-server) — for an HTTP/SSE surface
  instead of an SDK provider.
- [adiled.github.io/hum](https://adiled.github.io/hum/) — docs site.

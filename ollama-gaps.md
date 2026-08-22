# ollama-server: Gap Analysis — Missing Endpoints & Fields

> Goal: `ollama-server` should be a drop-in replacement for Ollama's API surface so
> `humd` can sit on port `11434` and any Ollama client (open-webui, cline, langchain, etc.)
> works without changes.

## Current endpoints

| Endpoint | Status |
|---|---|
| `GET /` | ✅ |
| `GET /api/tags` | ✅ (synthesized) |
| `POST /api/chat` | ✅ |
| `POST /api/generate` | ✅ |

## Missing core endpoints

| # | Endpoint | Method | Description | Priority |
|---|----------|--------|-------------|----------|
| 1 | `/api/version` | `GET` | Returns `{"version": "0.5.1"}` — some clients check this | 🔴 High |
| 2 | `/api/show` | `POST` | Model info: `modelfile`, `parameters`, `template`, `details`, `model_info`, `capabilities` — critical for RAG/agents that inspect models | 🔴 High |
| 3 | `/api/pull` | `POST` | Pull/download a model from Ollama library — streaming progress, resumable | 🟡 Medium |
| 4 | `/api/push` | `POST` | Push a model to a model library — streaming progress | 🟡 Medium |
| 5 | `/api/create` | `POST` | Create a model from another model, safetensors dir, or GGUF file — streaming progress | 🟡 Medium |
| 6 | `HEAD /api/blobs/:digest` | `HEAD` | Check if a blob exists on the server | 🟡 Medium |
| 7 | `POST /api/blobs/:digest` | `POST` | Push a file blob to the server (used by `/api/create`) | 🟡 Medium |
| 8 | `/api/copy` | `POST` | Copy/rename a model (`source` → `destination`) | 🟢 Low |
| 9 | `/api/delete` | `DELETE` | Delete a model and its data | 🟢 Low |
| 10 | `/api/ps` | `GET` | List currently loaded-in-memory models (with `expires_at`, `size_vram`) | 🔴 High |
| 11 | `/api/embed` | `POST` | Generate embeddings from a model. Returns per-input embeddings array + `total_duration`, `load_duration`, `prompt_eval_count` | 🔴 High (RAG support) |
| 12 | `/api/embeddings` | `POST` | Deprecated endpoint but still used by older clients. Returns `embedding` (single array) | 🟢 Low |

## Missing request/response fields on **existing** endpoints

### `POST /api/chat`

The Ollama chat spec supports fields the current implementation doesn't handle:

| Field | Direction | Description | Priority |
|-------|-----------|-------------|----------|
| `think` | ✅ req | For thinking models: should the model think before responding? | 🔴 High |
| `format` | ✅ req | `json` or JSON schema for structured outputs | 🔴 High |
| `keep_alive` | ✅ req | Controls how long model stays loaded (`0` = unload, `5m` = default, `never` = stay) | 🔴 High |
| `images` | ✅ req | Base64-encoded images in a message (multimodal) | 🟡 Medium |
| `messages[].tool_name` | ✅ req | Tool call result message: adds tool name to inform model of result | 🟡 Medium |
| `messages[].tool_calls` | ✅ req | Client sends back tool call results in message history | 🟡 Medium |
| `options.top_logprobs` | ✅ req | Number of log probs to return | 🟢 Low |
| `options` (full schema) | ✅ req | All model params: `num_keep`, `seed`, `num_predict`, `draft_num_predict`, `top_k`, `top_p`, `min_p`, `typical_p`, `repeat_last_n`, `temperature`, `repeat_penalty`, `presence_penalty`, `frequency_penalty`, `penalize_newline`, `stop`, `numa`, `num_ctx`, `num_batch`, `num_gpu`, `main_gpu`, `use_mmap`, `num_thread` | 🟡 Medium |
| `response.thinking` | ❌ resp | For thinking models — the model's thinking process | 🔴 High |
| `response.tool_name` | ❌ resp | Present when model returns a tool call | 🟡 Medium |
| `total_duration` | ❌ resp | Nanoseconds generating the response | 🟡 Medium |
| `load_duration` | ❌ resp | Nanoseconds loading the model | 🟡 Medium |
| `prompt_eval_count` | ❌ resp | Token count in prompt | 🟡 Medium |
| `prompt_eval_duration` | ❌ resp | Nanoseconds evaluating prompt | 🟡 Medium |
| `eval_count` | ❌ resp | Token count in response | 🟡 Medium |
| `eval_duration` | ❌ resp | Nanoseconds generating response | 🟡 Medium |
| `context` | ❌ resp | Conversation memory encoding (deprecated) | 🟢 Low |
| `total_duration` in stream chunks | ❌ resp | Should appear in the final chunk | ✅ Already handled in finish? |

### `POST /api/generate`

Same `options`, `think`, `format`, `keep_alive`, `images` + additional:

| Field | Direction | Description | Priority |
|-------|-----------|-------------|----------|
| `suffix` | ✅ req | Text after the model response | 🟡 Medium |
| `raw` | ✅ req | Bypass prompt templating, no `context` returned | 🟡 Medium |
| Image gen params (exp) | ✅ req | `width`, `height`, `steps` — for image generation models | 🟢 Low |

## Missing: Model management concept

The Ollama API treats models as first-class entities (list, show, pull, push, copy, delete, create).
The current `ollama-server` only has a static list of models configured in `OLLAMA_SERVER_MODELS`.
For a realistic gateway, we need:

1. **Dynamic model registry** — models can be added/discovered (from humd's cell pool, or from a configured list)
2. **`/api/show` needs real model metadata** — `modelfile`, `template`, `parameters`, `model_info`, `capabilities`
3. **Model lifecycle** — at minimum `keep_alive` should control model persistence

## Implementation notes

### `/api/version`
Trivial — return the crate version or a fixed Ollama-compatible string (e.g. `"0.5.0"`).

### `/api/show`
Needs to return per-model metadata. The response shape includes:
```json
{
  "modelfile": "...",
  "parameters": "num_keep 24\nstop ...\n",
  "template": "template string...",
  "details": { "parent_model": "", "format": "gguf", ... },
  "model_info": { "general.architecture": "llama", ... },
  "capabilities": ["completion"]
}
```
This is configuration/data — not a thrum interaction. Can be served from a models registry.

### `/api/ps`
Needs live state tracking — which models are currently loaded. This requires
humd to track model load/unload events and expose them.

### `/api/embed` / `/api/embeddings`
Needs a separate embedding path. Could:
- Delegate to a humd cell if one's configured as an embedding model
- Or return an error with a clear message that embedding isn't configured
- The response shape is straightforward: `{"model": "...", "embeddings": [[...]]}`

### `/api/pull` / `/api/push` / `/api/create` / blob endpoints
These are Ollama-specific model management features. For now, returning HTTP 501
"Not Implemented" with a JSON error is acceptable. They're important for full
compatibility but not for the core routing use case.

### `think` / thinking models
Ollama 0.3+ supports models with a `thinking` field. The `message` response should
include `"thinking": "..."` when the model thinks. Also `think: true` in the
request tells the model to reason before responding.

### `format` / structured output
When `format: "json"` or a JSON schema is provided, the response should respect it.
This is primarily a prompt-templating concern — the current implementation passes messages
through without any format-aware handling.

### `keep_alive`
Controls when a model stays in memory. For a stateless proxy this matters less,
but clients (especially Ollama-native ones) rely on `keep_alive: 0` to unload models.
We should at least acknowledge it and include it in the final response to match
Ollama's round-trip behavior.

### Metrics/duration in response
Ollama returns `total_duration`, `load_duration`, `prompt_eval_count`,
`prompt_eval_duration`, `eval_count`, `eval_duration` in the final chunk.
The current implementation copies `usage` from humd but the field names
don't match Ollama's. We should map them and also track these locally.

## Priority breakdown

### P0 — Must-have for drop-in compatibility
- `GET /api/version`
- `POST /api/show`
- `GET /api/ps`
- `think` / `response.thinking`
- `format` (JSON / structured output)
- `keep_alive` (acknowledge + include in response)
- Response time metrics: `total_duration`, `load_duration`, `prompt_eval_count`, `eval_count`, etc.

### P1 — Important for broader client support
- `POST /api/embed`
- `POST /api/generate` → `suffix`, `raw`, `images`
- `POST /api/chat` → `images`, `messages[].tool_calls`, `messages[].tool_name`
- `options.*` parameter passthrough

### P2 — Nice to have
- `POST /api/pull`, `/api/push`
- `POST /api/create`, `HEAD/POST /api/blobs/:digest`
- `POST /api/copy`, `DELETE /api/delete`
- `POST /api/embeddings` (deprecated)

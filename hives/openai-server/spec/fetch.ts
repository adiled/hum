// The specs this hive is held to, fetched at a pinned commit.
//
// openai.json is OpenAI's OpenAPI cut down to what this hive serves: the
// operations of its routes and every schema they reach, since the whole
// is five megabytes. openresponses.json is the Open Responses spec whole.
// Run with `bun spec/fetch.ts`; move a commit below to take a newer spec.

import { writeFileSync } from "node:fs";
import { join } from "node:path";

const OPENAI = { repo: "openai/openai-openapi", commit: "26a299159673d20d5cf0faf10e691beed3fed52d", path: "openapi.json" };
const OPEN_RESPONSES = { repo: "openresponses/openresponses", commit: "8907bc3b7c4300ff5c21fd647cfde9929243afa1", path: "public/openapi/openapi.json" };

const SERVED: Array<[string, string]> = [
  ["/chat/completions", "post"],
  ["/completions", "post"],
  ["/embeddings", "post"],
  ["/models", "get"],
  ["/models/{model}", "get"],
  ["/responses", "post"],
  ["/responses/{response_id}", "get"],
  ["/responses/{response_id}", "delete"],
];

const STREAMED = ["CreateChatCompletionStreamResponse", "CreateCompletionResponse", "ResponseStreamEvent", "ErrorResponse"];

type Json = Record<string, unknown>;

async function fetched(source: { repo: string; commit: string; path: string }): Promise<Json> {
  const url = `https://raw.githubusercontent.com/${source.repo}/${source.commit}/${source.path}`;
  const res = await fetch(url);
  if (!res.ok) throw new Error(`${url}: ${res.status}`);
  return (await res.json()) as Json;
}

function refsIn(node: unknown, into: Set<string>): void {
  if (Array.isArray(node)) {
    for (const n of node) refsIn(n, into);
    return;
  }
  if (node === null || typeof node !== "object") return;
  for (const [k, v] of Object.entries(node)) {
    if (k === "$ref" && typeof v === "string" && v.startsWith("#/components/schemas/")) into.add(v.slice("#/components/schemas/".length));
    else refsIn(v, into);
  }
}

function jsonOnly(op: Json): Json {
  const body = (op.requestBody as Json | undefined)?.content as Json | undefined;
  const responses: Json = {};
  for (const [code, r] of Object.entries((op.responses ?? {}) as Record<string, Json>)) {
    const content = r.content as Record<string, Json> | undefined;
    const schema = content?.["application/json"]?.schema ?? content?.["text/event-stream"]?.schema;
    if (schema) responses[code] = { content: { "application/json": { schema } } };
  }
  return {
    ...(body?.["application/json"] ? { requestBody: { content: { "application/json": { schema: (body["application/json"] as Json).schema } } } } : {}),
    responses,
  };
}

function cut(spec: Json): Json {
  const paths: Record<string, Json> = {};
  for (const [path, method] of SERVED) {
    const op = ((spec.paths as Record<string, Json>)[path] as Record<string, Json>)[method];
    paths[path] = { ...(paths[path] ?? {}), [method]: jsonOnly(op) };
  }
  const all = (spec.components as Json).schemas as Record<string, unknown>;
  const reached = new Set<string>(STREAMED);
  refsIn(paths, reached);
  const stack = [...reached];
  while (stack.length > 0) {
    const name = stack.pop()!;
    const found = new Set<string>();
    refsIn(all[name], found);
    for (const n of found) if (!reached.has(n)) {
      reached.add(n);
      stack.push(n);
    }
  }
  const schemas = Object.fromEntries([...reached].sort().map((n) => [n, all[n]]));
  return { openapi: spec.openapi, info: spec.info, "x-hum-source": OPENAI, paths, components: { schemas } };
}

const here = import.meta.dir;
writeFileSync(join(here, "openai.json"), JSON.stringify(cut(await fetched(OPENAI))) + "\n");
writeFileSync(join(here, "openresponses.json"), JSON.stringify({ ...(await fetched(OPEN_RESPONSES)), "x-hum-source": OPEN_RESPONSES }) + "\n");

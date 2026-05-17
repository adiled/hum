import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { createHash, randomUUID } from "node:crypto";
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { ThrumClient } from "./thrum.ts";
import { OpenAITranslator } from "./transform.ts";

interface NestlingConfig {
  host?: string;
  port?: number;
  apiKey?: string;
  models?: string[];
}

function readConfigFile(): NestlingConfig {
  const path = join(homedir(), ".config", "hum", "nestlings", "openai-server.json");
  try {
    const raw = readFileSync(path, "utf8");
    const parsed = JSON.parse(raw) as NestlingConfig;
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch {
    return {};
  }
}

const fileConfig = readConfigFile();
// Model IDs advertised on /v1/models come from the nestling's per-kind
// config (~/.config/hum/nestlings/openai-server.json). When unset, the
// list is empty — /v1/models returns an empty array. The recipe that
// installs this nestling is responsible for seeding the model id-set
// it wants exposed; the nestling itself stays model-agnostic.
const MODEL_IDS: string[] = Array.isArray(fileConfig.models) ? fileConfig.models : [];

// Precedence: env > config file > built-in defaults.
const PORT = process.env.OPENAI_SERVER_PORT !== undefined
  ? parseInt(process.env.OPENAI_SERVER_PORT, 10)
  : (typeof fileConfig.port === "number" ? fileConfig.port : 14620);
const HOST = process.env.OPENAI_SERVER_HOST
  ?? fileConfig.host
  ?? "127.0.0.1";
const API_KEY = process.env.OPENAI_SERVER_API_KEY
  ?? fileConfig.apiKey
  ?? "";

function readBody(req: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    let body = "";
    req.on("data", (c) => { body += c.toString(); });
    req.on("end", () => resolve(body));
    req.on("error", reject);
  });
}

function unauthorized(res: ServerResponse): void {
  res.writeHead(401, { "Content-Type": "application/json" });
  res.end(JSON.stringify({ error: { message: "missing or bad bearer", type: "invalid_request_error" } }));
}

function bad(res: ServerResponse, msg: string): void {
  res.writeHead(400, { "Content-Type": "application/json" });
  res.end(JSON.stringify({ error: { message: msg, type: "invalid_request_error" } }));
}

function checkAuth(req: IncomingMessage): boolean {
  if (!API_KEY) return true; // unauthenticated mode for local dev
  const auth = req.headers["authorization"];
  if (typeof auth !== "string") return false;
  const [scheme, token] = auth.split(" ");
  return scheme === "Bearer" && token === API_KEY;
}

interface OpenAIMessage {
  role: "system" | "user" | "assistant" | "tool";
  content?: string | Array<{ type: string; text?: string }>;
  tool_calls?: Array<{ id: string; function: { name: string; arguments: string } }>;
  tool_call_id?: string;
  name?: string;
}

interface OpenAITool {
  type: "function";
  function: {
    name: string;
    description?: string;
    parameters?: Record<string, unknown>;
  };
}

interface ToolSpec {
  name: string;
  description?: string;
  parameters?: Record<string, unknown>;
}

function toolsFromOpenAI(tools: OpenAITool[] | undefined): ToolSpec[] | undefined {
  if (!Array.isArray(tools) || tools.length === 0) return undefined;
  const out: ToolSpec[] = [];
  for (const t of tools) {
    if (t?.type !== "function" || !t.function?.name) continue;
    out.push({
      name: t.function.name,
      ...(t.function.description ? { description: t.function.description } : {}),
      ...(t.function.parameters ? { parameters: t.function.parameters } : {}),
    });
  }
  return out.length > 0 ? out : undefined;
}

function flatten(content: OpenAIMessage["content"]): string {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) return content.filter(p => p.type === "text").map(p => p.text ?? "").join("\n");
  return "";
}

interface ThrumAttachment {
  kind: string;
  mediaType: string;
  data?: string;
  url?: string;
}

// Pull non-text parts out of a content array and translate them to
// thrum-shape attachments. Today: image_url (both data: URIs and
// http(s) URLs). Future: input_audio, file refs.
function attachmentsFromContent(content: OpenAIMessage["content"]): ThrumAttachment[] {
  if (!Array.isArray(content)) return [];
  const out: ThrumAttachment[] = [];
  for (const part of content) {
    if (part.type === "image_url") {
      const url = (part as { image_url?: { url?: string } }).image_url?.url;
      if (!url) continue;
      if (url.startsWith("data:")) {
        // data:<media-type>;base64,<payload>
        const m = url.match(/^data:([^;]+);base64,(.+)$/);
        if (m) out.push({ kind: "image", mediaType: m[1], data: m[2] });
      } else {
        out.push({ kind: "image", mediaType: "image/*", url });
      }
    }
  }
  return out;
}

// Collect attachments from every message in the conversation (not
// just the last). hum's perch sees the union; multi-turn vision is
// then a perch-side concern (whichever perch knows how to interleave
// image blocks with text turns).
function allAttachments(messages: OpenAIMessage[]): ThrumAttachment[] {
  const out: ThrumAttachment[] = [];
  for (const m of messages) out.push(...attachmentsFromContent(m.content));
  return out;
}

// The OpenAI chat-completions wire is stateless: every request carries
// the full conversation. Whatever perch humd picks behind the prompt
// may or may not retain state — that's the perch's propensity, not
// this nestling's concern. The neutral, always-correct move is to
// forward the entire transcript every call; perches that are stateful
// will see a redundant prefix and respond just fine, perches that are
// stateless will get the context they need.
//
// (A future revision can opt in to delta-mode when humd's hello-ack
//  announces a stateful propensity for the target nest. Until that
//  wire piece lands, neutrality wins.)
function messagesToPrompt(messages: OpenAIMessage[]): { systemPrompt?: string; userPrompt: string } {
  const systemPieces: string[] = [];
  const turns: string[] = [];
  for (const msg of messages) {
    if (msg.role === "system") {
      systemPieces.push(flatten(msg.content));
    } else if (msg.role === "user") {
      turns.push(`User: ${flatten(msg.content)}`);
    } else if (msg.role === "assistant") {
      const text = flatten(msg.content);
      if (text) turns.push(`Assistant: ${text}`);
    }
    // role:"tool" handled separately via trailingToolReturns.
  }
  // Single user turn — emit verbatim; the "User:" label only helps
  // disambiguate when there's prior history.
  const userTurnCount = messages.filter(m => m.role === "user").length;
  const assistantTurnCount = messages.filter(m => m.role === "assistant").length;
  const single = userTurnCount === 1 && assistantTurnCount === 0;
  return {
    systemPrompt: systemPieces.length > 0 ? systemPieces.join("\n\n") : undefined,
    userPrompt: single ? (turns[0]?.replace(/^User: /, "") ?? "") : turns.join("\n\n"),
  };
}

// Stable sid keyed on the conversation anchor — same OC chat lands
// on the same hum sid across turns. Lets stateful perches reuse a
// roost when humd's pool keeps one warm; stateless perches just ignore
// the repeat. Either way the sid is meaningful, not random.
function sessionKey(messages: OpenAIMessage[]): string {
  const firstUser = messages.find(m => m.role === "user");
  const anchor = firstUser ? flatten(firstUser.content) : `none-${Date.now()}`;
  return createHash("sha1").update(anchor).digest("hex").slice(0, 16);
}

interface ToolReturn { tool_call_id: string; result: string; }

// A continuation request carries the prior tool_calls plus their
// answers as role:"tool" messages. Hum's daemon is parked inside
// execNestlerTool waiting for chi:"tool-result" — collect the
// trailing tool messages and forward them.
function trailingToolReturns(messages: OpenAIMessage[]): ToolReturn[] {
  const out: ToolReturn[] = [];
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i];
    if (m.role === "tool" && m.tool_call_id) {
      out.unshift({ tool_call_id: m.tool_call_id, result: flatten(m.content) });
    } else if (m.role !== "tool") break;
  }
  return out;
}

function hasTrailingUserAfterTool(messages: OpenAIMessage[]): boolean {
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i];
    if (m.role === "user") return true;
    if (m.role === "tool" || m.role === "assistant") return false;
  }
  return false;
}

const thrum = new ThrumClient();

async function start(): Promise<void> {
  const server = createServer(async (req, res) => {
    const url = new URL(req.url ?? "/", `http://${req.headers.host ?? "localhost"}`);

    if (req.method === "GET" && url.pathname === "/v1/models") {
      if (!checkAuth(req)) return unauthorized(res);
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify({
        object: "list",
        data: MODEL_IDS.map(id => ({ id, object: "model", created: 0, owned_by: "hum" })),
      }));
      return;
    }

    // /v1/models/{id} — single-model GET. Returns 404 when the id
    // isn't in our advertised set.
    if (req.method === "GET" && url.pathname.startsWith("/v1/models/")) {
      if (!checkAuth(req)) return unauthorized(res);
      const id = decodeURIComponent(url.pathname.slice("/v1/models/".length));
      if (!MODEL_IDS.includes(id)) {
        res.writeHead(404, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ error: { message: `model '${id}' not found`, type: "invalid_request_error" } }));
        return;
      }
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify({ id, object: "model", created: 0, owned_by: "hum" }));
      return;
    }

    if (req.method === "POST" && url.pathname === "/v1/chat/completions") {
      if (!checkAuth(req)) return unauthorized(res);
      let body: {
        messages?: OpenAIMessage[];
        model?: string;
        stream?: boolean;
        user?: string;
        tools?: OpenAITool[];
        // Pass-through sampling knobs. nest doesn't act on them; perches
        // that honor them (future native-API perches) read from the
        // chi:"prompt" tone they're forwarded on.
        temperature?: number;
        top_p?: number;
        max_completion_tokens?: number;
        max_tokens?: number;
        stop?: string | string[];
        seed?: number;
        n?: number;
        frequency_penalty?: number;
        presence_penalty?: number;
        tool_choice?: string | { type: string; function?: { name: string } };
        parallel_tool_calls?: boolean;
        response_format?: { type: string; json_schema?: { name?: string; schema?: unknown; strict?: boolean } };
        logprobs?: boolean;
        top_logprobs?: number;
        stream_options?: { include_usage?: boolean };
      };
      try { body = JSON.parse(await readBody(req)); } catch { return bad(res, "invalid JSON body"); }
      const messages = body.messages ?? [];
      if (messages.length === 0) return bad(res, "messages required");

      // OpenAI's `n` requests multiple completions per call — we serve
      // one perch session per prompt, so reject explicitly instead of
      // silently returning n=1.
      if (typeof body.n === "number" && body.n > 1) {
        return bad(res, `n>1 unsupported (this server serves a single completion per call)`);
      }
      // logprobs / top_logprobs aren't emitted by the perches we
      // ship today. Spec-compliant explicit reject beats silent ignore.
      if (body.logprobs === true || (typeof body.top_logprobs === "number" && body.top_logprobs > 0)) {
        return bad(res, "logprobs unsupported by hum perches (claude-cli doesn't emit token probabilities)");
      }

      const stream = body.stream !== false; // default to streaming
      const includeUsage = body.stream_options?.include_usage !== false;
      // body.model is the only correct source — the client picks. If
      // absent, fall back to the first advertised id; if none, use a
      // pass-through tag humd will reject loudly rather than guess.
      const model = body.model ?? MODEL_IDS[0] ?? "unspecified";
      // body.user wins when the client supplies a session id; otherwise
      // derive a stable one from the conversation anchor.
      const sid = body.user ?? `oai-${sessionKey(messages)}`;
      let { systemPrompt, userPrompt } = messagesToPrompt(messages);
      const tools = toolsFromOpenAI(body.tools);
      const attachments = allAttachments(messages);

      // response_format: JSON mode. OpenAI's contract is "the model is
      // constrained to emit valid JSON." We inject the constraint into
      // the system prompt — model-side enforcement (no grammar lock
      // available across all perches). json_schema gets the schema
      // shown verbatim so the model can mirror it.
      if (body.response_format && body.response_format.type !== "text") {
        const fmt = body.response_format;
        let jsonHint = "Respond with valid JSON only. No prose, no markdown fences.";
        if (fmt.type === "json_schema" && fmt.json_schema?.schema) {
          jsonHint += `\nConform to this JSON Schema:\n${JSON.stringify(fmt.json_schema.schema, null, 2)}`;
        }
        systemPrompt = systemPrompt ? `${systemPrompt}\n\n${jsonHint}` : jsonHint;
      }

      // Sampling/limit knobs — pass to humd via a sampling block so
      // perches that honor them (anthropic-native, ollama, etc.) can.
      // claude-cli today ignores them; that's fine, they're optional.
      const sampling: Record<string, unknown> = {};
      if (typeof body.temperature === "number") sampling.temperature = body.temperature;
      if (typeof body.top_p === "number") sampling.topP = body.top_p;
      const maxTokens = body.max_completion_tokens ?? body.max_tokens;
      if (typeof maxTokens === "number") sampling.maxTokens = maxTokens;
      if (body.stop !== undefined) sampling.stop = body.stop;
      if (typeof body.seed === "number") sampling.seed = body.seed;
      if (typeof body.frequency_penalty === "number") sampling.frequencyPenalty = body.frequency_penalty;
      if (typeof body.presence_penalty === "number") sampling.presencePenalty = body.presence_penalty;
      if (body.tool_choice !== undefined) sampling.toolChoice = body.tool_choice;
      if (typeof body.parallel_tool_calls === "boolean") sampling.parallelToolCalls = body.parallel_tool_calls;

      const chunkId = `chatcmpl-${randomUUID()}`;
      const translator = new OpenAITranslator(chunkId, model, includeUsage);

      if (stream) {
        res.writeHead(200, {
          "Content-Type": "text/event-stream",
          "Cache-Control": "no-cache",
          "Connection": "keep-alive",
          "X-Accel-Buffering": "no",
        });
        thrum.on(sid, (msg) => {
          const frames = translator.ingest(msg);
          for (const f of frames) {
            if (f === "data: [DONE]\n\n") {
              res.write(f);
              res.end();
              thrum.off(sid);
              return;
            }
            res.write(f);
          }
        });
      } else {
        // Non-streaming: accumulate streamed deltas, fold into a single
        // chat completion response when [DONE] arrives. Spec-compliant
        // OpenAI shape — clients that opt out of SSE get one JSON body.
        let accumulatedContent = "";
        let accumulatedReasoning = "";
        const accumulatedToolCalls: Array<{
          id: string;
          type: string;
          function: { name: string; arguments: string };
        }> = [];
        let finishReason: string | null = "stop";
        let usage: Record<string, number> | undefined;
        thrum.on(sid, (msg) => {
          const frames = translator.ingest(msg);
          for (const f of frames) {
            if (f === "data: [DONE]\n\n") {
              const body: Record<string, unknown> = {
                id: chunkId,
                object: "chat.completion",
                created: Math.floor(Date.now() / 1000),
                model,
                choices: [{
                  index: 0,
                  message: {
                    role: "assistant",
                    content: accumulatedContent || null,
                    ...(accumulatedReasoning ? { reasoning_content: accumulatedReasoning } : {}),
                    ...(accumulatedToolCalls.length > 0 ? { tool_calls: accumulatedToolCalls } : {}),
                  },
                  finish_reason: finishReason,
                }],
              };
              if (usage) body.usage = usage;
              res.writeHead(200, { "Content-Type": "application/json" });
              res.end(JSON.stringify(body));
              thrum.off(sid);
              return;
            }
            // Parse the SSE frame to fold into the accumulator.
            const m = f.match(/^data: (.+)\n\n$/);
            if (!m) continue;
            try {
              const chunk = JSON.parse(m[1]) as {
                choices?: Array<{ delta?: Record<string, unknown>; finish_reason?: string | null }>;
                usage?: Record<string, number>;
              };
              if (chunk.usage) usage = chunk.usage;
              const choice = chunk.choices?.[0];
              if (!choice) continue;
              if (choice.finish_reason) finishReason = choice.finish_reason;
              const d = choice.delta ?? {};
              if (typeof d.content === "string") accumulatedContent += d.content;
              if (typeof d.reasoning_content === "string") accumulatedReasoning += d.reasoning_content;
              const tcs = d.tool_calls as Array<{
                index: number;
                id?: string;
                type?: string;
                function?: { name?: string; arguments?: string };
              }> | undefined;
              if (Array.isArray(tcs)) {
                for (const tc of tcs) {
                  const slot = accumulatedToolCalls[tc.index] ?? {
                    id: "",
                    type: "function",
                    function: { name: "", arguments: "" },
                  };
                  if (tc.id) slot.id = tc.id;
                  if (tc.type) slot.type = tc.type;
                  if (tc.function?.name) slot.function.name = tc.function.name;
                  if (tc.function?.arguments) slot.function.arguments += tc.function.arguments;
                  accumulatedToolCalls[tc.index] = slot;
                }
              }
            } catch {}
          }
        });
      }

      req.on("close", () => {
        thrum.off(sid);
        // Don't cancel on disconnect when the daemon is mid-tool — the
        // model is parked awaiting a tool-result, not actively generating.
      });

      const toolReturns = trailingToolReturns(messages);
      for (const tr of toolReturns) {
        thrum.send({ chi: "tool-result", sid, callId: tr.tool_call_id, result: tr.result });
      }

      const sendPrompt = hasTrailingUserAfterTool(messages) || toolReturns.length === 0;
      if (sendPrompt) {
        thrum.send({
          chi: "prompt",
          sid,
          nestling: "openai-server",
          modelId: model,
          content: userPrompt,
          ...(systemPrompt ? { systemPrompt } : {}),
          ...(tools ? { tools } : {}),
          ...(attachments.length > 0 ? { attachments } : {}),
          ...(Object.keys(sampling).length > 0 ? { sampling } : {}),
        });
      }
      return;
    }

    if (req.method === "GET" && url.pathname === "/") {
      res.writeHead(200, { "Content-Type": "text/plain" });
      res.end("hum-openai-server\n");
      return;
    }

    res.writeHead(404);
    res.end();
  });

  await new Promise<void>((resolve) => {
    server.listen(PORT, HOST, () => resolve());
  });

  // Determine the real bound address (handles port: 0 / wildcard host).
  const addr = server.address();
  const actualHost = (addr && typeof addr === "object" && addr.address) ? addr.address : HOST;
  const actualPort = (addr && typeof addr === "object" && typeof addr.port === "number") ? addr.port : PORT;
  console.log(`[hum-openai-server] listening on http://${actualHost}:${actualPort}`);

  await thrum.connect({ host: actualHost, port: actualPort, scheme: "http" });
  console.log(`[hum-openai-server] connected to thrum`);
}

start().catch(e => { console.error("[hum-openai-server] startup failed:", e); process.exit(1); });

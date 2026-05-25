import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { createHash, randomUUID } from "node:crypto";
import { readFileSync, appendFileSync, mkdirSync } from "node:fs";
import { homedir } from "node:os";
import { join, dirname } from "node:path";
import { ThrumClient } from "./thrum.ts";
import { OpenAITranslator } from "./transform.ts";
import { toolsFromOpenAI, type ToolSpec, type OpenAITool } from "./tools.ts";
export { toolsFromOpenAI } from "./tools.ts";

interface BeeConfig {
  host?: string;
  port?: number;
  apiKey?: string;
  models?: string[];
}

function readConfigFile(): BeeConfig {
  const path = join(homedir(), ".config", "hum", "hives", "openai-server.json");
  try {
    const raw = readFileSync(path, "utf8");
    const parsed = JSON.parse(raw) as BeeConfig;
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch {
    return {};
  }
}

const fileConfig = readConfigFile();
// Model IDs advertised on /v1/models come from the bee's per-kind
// config (~/.config/hum/hives/openai-server.json). When unset, the
// list is empty — /v1/models returns an empty array. The recipe that
// installs this bee is responsible for seeding the model id-set
// it wants exposed; the bee itself stays model-agnostic.
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

// ── tenant + audit + usage + rate-limit ──────────────────────────────
// Lightweight gateway concerns. None of these are kernel-level — they
// live in the bee because the wire format (OpenAI shape) is
// where multi-tenant routing, per-tenant billing, audit trails, and
// quota enforcement belong. hum's kernel stays format-neutral.

const STATE_DIR = (process.env.XDG_STATE_HOME ?? join(homedir(), ".local/state")) + "/hum/openai-server";
const AUDIT_LOG = join(STATE_DIR, "audit.log");
const USAGE_PATH = join(STATE_DIR, "usage.json");
try { mkdirSync(STATE_DIR, { recursive: true }); } catch {}

function tenantOf(req: IncomingMessage): string {
  const h = req.headers["x-tenant"];
  if (typeof h === "string" && h.length > 0) return h.replace(/[^A-Za-z0-9_-]/g, "");
  return "default";
}

function audit(entry: Record<string, unknown>): void {
  const line = JSON.stringify({ ts: new Date().toISOString(), ...entry }) + "\n";
  try { appendFileSync(AUDIT_LOG, line); } catch {}
}

interface TenantUsage {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  requests: number;
}
const USAGE: Record<string, TenantUsage> = (() => {
  try { return JSON.parse(readFileSync(USAGE_PATH, "utf8")); } catch { return {}; }
})();

// sid → claude session_id. Populated from chi:"session-ready" so
// follow-up turns can ship `resume` in chi:"prompt".
const sid_to_nestid: Map<string, string> = new Map();

// responseId → sid. Lets callers chain via OpenAI Responses
// `previous_response_id` even though hum's sid is independent.
// Capped to avoid unbounded growth; oldest entries evicted FIFO.
const bloom_to_sid: Map<string, string> = new Map();

// sid → caller-owned bookkeeping (metadata, safety_identifier,
// prompt_cache_key). Echoed back on response.completed per spec.
interface SidMeta {
  metadata?: Record<string, string>;
  safety_identifier?: string;
  prompt_cache_key?: string;
}
const sid_to_meta: Map<string, SidMeta> = new Map();
const SID_META_CAP = 4096;
function rememberSidMeta(sid: string, meta: SidMeta): void {
  if (Object.keys(meta).length === 0) return;
  if (sid_to_meta.size >= SID_META_CAP) {
    const first = sid_to_meta.keys().next().value;
    if (first) sid_to_meta.delete(first);
  }
  sid_to_meta.set(sid, meta);
}
const RESPONSE_MAP_CAP = 4096;
function rememberResponse(responseId: string, sid: string): void {
  if (bloom_to_sid.size >= RESPONSE_MAP_CAP) {
    const first = bloom_to_sid.keys().next().value;
    if (first) bloom_to_sid.delete(first);
  }
  bloom_to_sid.set(responseId, sid);
}
let usageDirty = false;
function trackUsage(tenant: string, prompt: number, completion: number): void {
  const u = USAGE[tenant] ?? { promptTokens: 0, completionTokens: 0, totalTokens: 0, requests: 0 };
  u.promptTokens += prompt;
  u.completionTokens += completion;
  u.totalTokens += prompt + completion;
  u.requests += 1;
  USAGE[tenant] = u;
  usageDirty = true;
}
// Flush usage to disk every 30s. Atomicity not critical — counters
// are monotonic, a missed update at most undercounts on crash.
setInterval(() => {
  if (!usageDirty) return;
  try {
    appendFileSync(USAGE_PATH + ".tmp", JSON.stringify(USAGE, null, 2));
    // Best-effort atomic-ish rename.
    require("node:fs").renameSync(USAGE_PATH + ".tmp", USAGE_PATH);
    usageDirty = false;
  } catch {}
}, 30_000).unref();

// Token bucket per tenant. Default 60 requests/min, configurable
// via per-tenant config later. Capacity = burst, refill = sustained.
interface Bucket { tokens: number; lastRefill: number; }
const BUCKETS: Record<string, Bucket> = {};
const RATE_CAPACITY = parseInt(process.env.OPENAI_SERVER_RATE_CAPACITY ?? "60", 10);
const RATE_REFILL_PER_SEC = parseFloat(process.env.OPENAI_SERVER_RATE_REFILL ?? "1.0");
function allow(tenant: string): boolean {
  const now = Date.now();
  const b = BUCKETS[tenant] ?? { tokens: RATE_CAPACITY, lastRefill: now };
  const elapsed = (now - b.lastRefill) / 1000;
  b.tokens = Math.min(RATE_CAPACITY, b.tokens + elapsed * RATE_REFILL_PER_SEC);
  b.lastRefill = now;
  if (b.tokens < 1) { BUCKETS[tenant] = b; return false; }
  b.tokens -= 1;
  BUCKETS[tenant] = b;
  return true;
}
function tooManyRequests(res: ServerResponse, tenant: string): void {
  res.writeHead(429, {
    "Content-Type": "application/json",
    "Retry-After": "60",
    "X-RateLimit-Tenant": tenant,
  });
  res.end(JSON.stringify({ error: { message: `rate limit exceeded for tenant '${tenant}'`, type: "rate_limit_exceeded" } }));
}

interface OpenAIMessage {
  role: "system" | "user" | "assistant" | "tool";
  content?: string | Array<{ type: string; text?: string }>;
  tool_calls?: Array<{ id: string; function: { name: string; arguments: string } }>;
  tool_call_id?: string;
  name?: string;
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
// just the last). hum's worker sees the union; multi-turn vision is
// then a worker-side concern (whichever worker knows how to interleave
// image blocks with text turns).
function allAttachments(messages: OpenAIMessage[]): ThrumAttachment[] {
  const out: ThrumAttachment[] = [];
  for (const m of messages) out.push(...attachmentsFromContent(m.content));
  return out;
}

// The OpenAI chat-completions wire is stateless: every request carries
// the full conversation. Whatever worker humd picks behind the prompt
// may or may not retain state — that's the worker's propensity, not
// this bee's concern. The neutral, always-correct move is to
// forward the entire transcript every call; workers that are stateful
// will see a redundant prefix and respond just fine, workers that are
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
// on the same hum sid across turns. Lets stateful workers reuse a
// cell when humd's pool keeps one warm; stateless workers just ignore
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
      const tenant = tenantOf(req);
      if (!allow(tenant)) return tooManyRequests(res, tenant);
      let body: {
        messages?: OpenAIMessage[];
        model?: string;
        stream?: boolean;
        user?: string;
        tools?: OpenAITool[];
        // Pass-through sampling knobs. nest doesn't act on them; workers
        // that honor them (future native-API workers) read from the
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
      // one worker session per prompt, so reject explicitly instead of
      // silently returning n=1.
      if (typeof body.n === "number" && body.n > 1) {
        return bad(res, `n>1 unsupported (this server serves a single completion per call)`);
      }
      // logprobs / top_logprobs aren't emitted by the workers we
      // ship today. Spec-compliant explicit reject beats silent ignore.
      if (body.logprobs === true || (typeof body.top_logprobs === "number" && body.top_logprobs > 0)) {
        return bad(res, "logprobs unsupported by hum workers (claude-cli doesn't emit token probabilities)");
      }

      const stream = body.stream !== false; // default to streaming
      const includeUsage = body.stream_options?.include_usage !== false;
      // body.model is the only correct source — the client picks. If
      // absent, fall back to the first advertised id; if none, use a
      // pass-through tag humd will reject loudly rather than guess.
      const model = body.model ?? MODEL_IDS[0] ?? "unspecified";
      // body.user wins when the client supplies a session id; otherwise
      // derive a stable one from the conversation anchor. Tenant is
      // prefixed so different tenants never collide on the same sid.
      const sid = `${tenant === "default" ? "" : tenant + ":"}${body.user ?? `oai-${sessionKey(messages)}`}`;
      audit({ endpoint: "chat.completions", tenant, model, sid, stream: body.stream ?? true });
      let { systemPrompt, userPrompt } = messagesToPrompt(messages);
      const tools = toolsFromOpenAI(body.tools);
      const attachments = allAttachments(messages);

      // response_format: JSON mode. OpenAI's contract is "the model is
      // constrained to emit valid JSON." We inject the constraint into
      // the system prompt — model-side enforcement (no grammar lock
      // available across all workers). json_schema gets the schema
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
      // workers that honor them (anthropic-native, ollama, etc.) can.
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
            // Capture per-tenant usage from the dedicated usage frame.
            const um = f.match(/^data: (.+)\n\n$/);
            if (um) {
              try {
                const parsed = JSON.parse(um[1]) as { usage?: { prompt_tokens?: number; completion_tokens?: number } };
                if (parsed.usage) {
                  trackUsage(tenant, parsed.usage.prompt_tokens ?? 0, parsed.usage.completion_tokens ?? 0);
                }
              } catch {}
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
              if (usage) {
                body.usage = usage;
                trackUsage(tenant, usage.prompt_tokens ?? 0, usage.completion_tokens ?? 0);
              }
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
          hive: "openai-server",
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

    // ── /v1/responses — OpenAI's newer state-aware API ────────────────
    // Translates Responses-shape I/O into the same thrum flow as
    // chat/completions. State continuity rides on hum's session sid
    // (derived from `previous_response_id` when given, else from
    // input). Streaming emits Responses-shape SSE events; non-stream
    // returns the single-object Response body.
    if (req.method === "POST" && url.pathname === "/v1/responses") {
      if (!checkAuth(req)) return unauthorized(res);
      const tenant = tenantOf(req);
      if (!allow(tenant)) return tooManyRequests(res, tenant);
      type ResponsesInputItem =
        | { role?: string; content?: string | Array<{ type: string; text?: string; image_url?: { url?: string } }> }
        | { type: "function_call"; call_id?: string; name?: string; arguments?: string }
        | { type: "function_call_output"; call_id?: string; output?: string }
        | { type: "mcp_call"; id?: string; server_label?: string; name?: string; arguments?: string; output?: string };
      let body: {
        model?: string;
        input?: string | Array<ResponsesInputItem>;
        instructions?: string;
        stream?: boolean;
        previous_response_id?: string;
        conversation?: string | { id: string } | null;
        user?: string;
        metadata?: Record<string, string>;
        safety_identifier?: string;
        prompt_cache_key?: string;
        prompt_cache_retention?: string;
        store?: boolean;
        allowed_tools?: string[];
        service_tier?: string;
        truncation?: string;
        reasoning?: { effort?: string; summary?: string };
        text?: { verbosity?: string };
        include?: string[];
        background?: boolean;
        stream_options?: Record<string, unknown>;
        max_output_tokens?: number;
        max_tool_calls?: number;
        temperature?: number;
        top_p?: number;
        top_logprobs?: number;
        frequency_penalty?: number;
        presence_penalty?: number;
        tools?: OpenAITool[];
        tool_choice?: string | { type: string };
        parallel_tool_calls?: boolean;
        response_format?: { type: string; json_schema?: unknown };
      };
      try { body = JSON.parse(await readBody(req)); } catch { return bad(res, "invalid JSON body"); }
      if (!body.input) return bad(res, "input required");

      // Two input modes per OpenAI Responses contract:
      //   - delta: previous_response_id present, input = new turn only
      //   - full-history: no previous_response_id, input = whole convo
      // sid derivation follows the same split so continuity binds
      // to a stable anchor without depending on out-of-spec headers.
      const stream = body.stream === true;
      const model = body.model ?? MODEL_IDS[0] ?? "unspecified";
      const isDeltaMode = typeof body.previous_response_id === "string" && body.previous_response_id.length > 0;

      // Walk the input items. In full-history mode we keep the FIRST
      // user message (sid anchor) and the LATEST (new turn text); in
      // delta mode we flatten every user-text item since input ≡ new
      // turn. Image attachments collected either way.
      let userText = "";
      let firstUserMsg = "";
      let firstItemId = "";
      const respAttachments: ThrumAttachment[] = [];
      if (typeof body.input === "string") {
        userText = body.input;
        firstUserMsg = body.input;
      } else if (Array.isArray(body.input)) {
        if (body.input.length > 0) {
          const first = body.input[0] as { id?: unknown };
          if (typeof first?.id === "string") firstItemId = first.id;
        }
        const allUserTurns: string[] = [];
        for (const item of body.input) {
          const it = item as ResponsesInputItem & { role?: string; type?: string };
          // Non-message item types: function_call, function_call_output,
          // mcp_call, reasoning, custom tool calls, apply_patch,
          // file_search_call, image_generation_call, provider extensions
          // (provider_slug:custom_type). All skipped — claude pulls
          // history via --resume; provider extensions are opaque.
          if (typeof it.type === "string" && it.type !== "message") continue;
          if ((it as { role?: string }).role !== "user" || !("content" in it)) continue;
          const content = (it as { content?: unknown }).content;
          let collected = "";
          if (typeof content === "string") {
            collected = content;
          } else if (Array.isArray(content)) {
            const parts: string[] = [];
            for (const p of content as Array<{ type: string; text?: string; image_url?: { url?: string } }>) {
              if (p.type === "input_text" || p.type === "text") {
                if (p.text) parts.push(p.text);
              } else if (p.type === "input_image" || p.type === "image_url") {
                const u = p.image_url?.url;
                if (u?.startsWith("data:")) {
                  const m = u.match(/^data:([^;]+);base64,(.+)$/);
                  if (m) respAttachments.push({ kind: "image", mediaType: m[1], data: m[2] });
                } else if (u) {
                  respAttachments.push({ kind: "image", mediaType: "image/*", url: u });
                }
              }
            }
            collected = parts.join("\n");
          }
          if (collected) allUserTurns.push(collected);
        }
        firstUserMsg = allUserTurns[0] ?? "";
        userText = isDeltaMode
          ? allUserTurns.join("\n")
          : (allUserTurns[allUserTurns.length - 1] ?? "");
      }

      // Sid resolver cascade:
      //   1. previous_response_id → bloom_to_sid lookup (or self-hash on miss)
      //   2. conversation          → canonical session anchor per spec
      //   3. metadata.session_id   → caller-explicit metadata convention
      //   4. firstItemId           → spec-required item id when present
      //   5. hash(firstUserMsg)    → fragile last-resort
      const prefix = `${tenant === "default" ? "" : tenant + ":"}oai-r-`;
      const hash16 = (s: string) => createHash("sha1").update(s).digest("hex").slice(0, 16);
      const conversationId = typeof body.conversation === "string"
        ? body.conversation
        : (body.conversation && typeof body.conversation === "object" && typeof body.conversation.id === "string")
          ? body.conversation.id
          : undefined;
      const metaSessionId = typeof body.metadata?.session_id === "string"
        ? body.metadata.session_id
        : undefined;
      let sid: string;
      if (isDeltaMode) {
        const known = bloom_to_sid.get(body.previous_response_id!);
        sid = known ?? `${prefix}${hash16(body.previous_response_id!)}`;
      } else if (conversationId && conversationId.length > 0) {
        sid = `${prefix}${hash16(conversationId)}`;
      } else if (metaSessionId && metaSessionId.length > 0) {
        sid = `${prefix}${hash16(metaSessionId)}`;
      } else if (firstItemId.length > 0) {
        sid = `${prefix}${hash16(firstItemId)}`;
      } else {
        sid = `${prefix}${hash16(firstUserMsg.slice(0, 256))}`;
      }
      const responseId = `resp_${randomUUID().replace(/-/g, "").slice(0, 24)}`;
      rememberResponse(responseId, sid);
      rememberSidMeta(sid, {
        ...(body.metadata && Object.keys(body.metadata).length > 0 ? { metadata: body.metadata } : {}),
        ...(body.safety_identifier ? { safety_identifier: body.safety_identifier } : {}),
        ...(body.prompt_cache_key ? { prompt_cache_key: body.prompt_cache_key } : {}),
      });
      audit({ endpoint: "responses", tenant, model, sid, responseId, stream });

      let systemPrompt = body.instructions;
      if (body.response_format && body.response_format.type !== "text") {
        const jsonHint = "Respond with valid JSON only. No prose, no markdown fences.";
        systemPrompt = systemPrompt ? `${systemPrompt}\n\n${jsonHint}` : jsonHint;
      }

      const sampling: Record<string, unknown> = {};
      if (typeof body.temperature === "number") sampling.temperature = body.temperature;
      if (typeof body.top_p === "number") sampling.topP = body.top_p;
      if (typeof body.max_output_tokens === "number") sampling.maxTokens = body.max_output_tokens;
      if (body.tool_choice !== undefined) sampling.toolChoice = body.tool_choice;
      if (typeof body.parallel_tool_calls === "boolean") sampling.parallelToolCalls = body.parallel_tool_calls;
      if (body.service_tier) sampling.serviceTier = body.service_tier;
      if (body.truncation) sampling.truncation = body.truncation;
      if (body.reasoning?.effort) sampling.reasoningEffort = body.reasoning.effort;
      if (body.reasoning?.summary) sampling.reasoningSummary = body.reasoning.summary;
      if (body.text?.verbosity) sampling.textVerbosity = body.text.verbosity;
      if (Array.isArray(body.include)) sampling.include = body.include;
      if (typeof body.max_tool_calls === "number") sampling.maxToolCalls = body.max_tool_calls;
      if (typeof body.frequency_penalty === "number") sampling.frequencyPenalty = body.frequency_penalty;
      if (typeof body.presence_penalty === "number") sampling.presencePenalty = body.presence_penalty;
      if (typeof body.top_logprobs === "number") sampling.topLogprobs = body.top_logprobs;

      const tools = toolsFromOpenAI(body.tools);
      const itemId = `msg_${randomUUID().replace(/-/g, "").slice(0, 24)}`;
      const createdAt = Math.floor(Date.now() / 1000);

      if (stream) {
        res.writeHead(200, {
          "Content-Type": "text/event-stream",
          "Cache-Control": "no-cache",
          "Connection": "keep-alive",
          "X-Accel-Buffering": "no",
        });
        let seq = 0;
        const sse = (event: string, data: Record<string, unknown>) => {
          const enriched = { ...data, sequence_number: seq++ };
          res.write(`event: ${event}\ndata: ${JSON.stringify(enriched)}\n\n`);
        };
        sse("response.created", { type: "response.created", response: { id: responseId, object: "response", created_at: createdAt, model, status: "in_progress" } });
        sse("response.in_progress", { type: "response.in_progress", response: { id: responseId, object: "response", created_at: createdAt, model, status: "in_progress" } });

        type OpenItem =
          | { kind: "reasoning"; outputIndex: number; itemId: string; text: string }
          | { kind: "message"; outputIndex: number; itemId: string; text: string };
        const openItems = new Map<number, OpenItem>();
        const finalOutput: Array<Record<string, unknown>> = [];
        let nextOutputIndex = 0;
        let usage: { input_tokens?: number; output_tokens?: number; cache_read_input_tokens?: number; cache_creation_input_tokens?: number } | undefined;

        const closeItem = (blockIdx: number) => {
          const open = openItems.get(blockIdx);
          if (!open) return;
          openItems.delete(blockIdx);
          if (open.kind === "reasoning") {
            sse("response.reasoning_summary_text.done", {
              type: "response.reasoning_summary_text.done",
              item_id: open.itemId,
              output_index: open.outputIndex,
              summary_index: 0,
              text: open.text,
            });
            sse("response.reasoning_summary_part.done", {
              type: "response.reasoning_summary_part.done",
              item_id: open.itemId,
              output_index: open.outputIndex,
              summary_index: 0,
              part: { type: "summary_text", text: open.text },
            });
            sse("response.reasoning.done", {
              type: "response.reasoning.done",
              item_id: open.itemId,
              output_index: open.outputIndex,
              content_index: 0,
              text: open.text,
            });
            const doneItem = {
              id: open.itemId,
              type: "reasoning",
              status: "completed",
              summary: [{ type: "summary_text", text: open.text }],
            };
            sse("response.output_item.done", {
              type: "response.output_item.done",
              output_index: open.outputIndex,
              item: doneItem,
            });
            finalOutput.push(doneItem);
          } else {
            sse("response.output_text.done", {
              type: "response.output_text.done",
              item_id: open.itemId,
              output_index: open.outputIndex,
              content_index: 0,
              text: open.text,
            });
            sse("response.content_part.done", {
              type: "response.content_part.done",
              item_id: open.itemId,
              output_index: open.outputIndex,
              content_index: 0,
              part: { type: "output_text", text: open.text },
            });
            const doneItem = {
              id: open.itemId,
              type: "message",
              role: "assistant",
              status: "completed",
              content: [{ type: "output_text", text: open.text }],
            };
            sse("response.output_item.done", {
              type: "response.output_item.done",
              output_index: open.outputIndex,
              item: doneItem,
            });
            finalOutput.push(doneItem);
          }
        };

        thrum.on(sid, (msg) => {
          const chi = msg.chi as string | undefined;
          const chunkType = msg.chunkType as string | undefined;
          const blockIdx = typeof msg.id === "number" ? (msg.id as number)
            : typeof msg.blockIdx === "number" ? (msg.blockIdx as number)
            : -1;
          if (chi === "session-ready") {
            const nid = msg.nestId as string | undefined;
            if (nid) sid_to_nestid.set(sid, nid);
            return;
          }
          if (chi === "chunk" && chunkType === "reasoning_start") {
            const idx = nextOutputIndex++;
            const rsId = `rs_${randomUUID().replace(/-/g, "").slice(0, 24)}`;
            openItems.set(blockIdx, { kind: "reasoning", outputIndex: idx, itemId: rsId, text: "" });
            sse("response.output_item.added", {
              type: "response.output_item.added",
              output_index: idx,
              item: { id: rsId, type: "reasoning", status: "in_progress", summary: [] },
            });
            sse("response.reasoning_summary_part.added", {
              type: "response.reasoning_summary_part.added",
              item_id: rsId,
              output_index: idx,
              summary_index: 0,
              part: { type: "summary_text", text: "" },
            });
            return;
          }
          if (chi === "chunk" && chunkType === "reasoning_delta") {
            const open = openItems.get(blockIdx);
            if (!open || open.kind !== "reasoning") return;
            const delta = (msg.delta as string) ?? "";
            if (!delta) return;
            open.text += delta;
            // Raw thinking text stream.
            sse("response.reasoning.delta", {
              type: "response.reasoning.delta",
              item_id: open.itemId,
              output_index: open.outputIndex,
              content_index: 0,
              delta,
            });
            // Summary-stream surface (what most OAI Responses
            // consumers key on for showing a reasoning panel).
            sse("response.reasoning_summary_text.delta", {
              type: "response.reasoning_summary_text.delta",
              item_id: open.itemId,
              output_index: open.outputIndex,
              summary_index: 0,
              delta,
            });
            return;
          }
          if (chi === "chunk" && chunkType === "text_start") {
            const idx = nextOutputIndex++;
            const msgId = `msg_${randomUUID().replace(/-/g, "").slice(0, 24)}`;
            openItems.set(blockIdx, { kind: "message", outputIndex: idx, itemId: msgId, text: "" });
            sse("response.output_item.added", {
              type: "response.output_item.added",
              output_index: idx,
              item: { id: msgId, type: "message", role: "assistant", status: "in_progress", content: [] },
            });
            sse("response.content_part.added", {
              type: "response.content_part.added",
              item_id: msgId,
              output_index: idx,
              content_index: 0,
              part: { type: "output_text", text: "" },
            });
            return;
          }
          if (chi === "chunk" && chunkType === "text_delta") {
            const open = openItems.get(blockIdx);
            if (!open || open.kind !== "message") {
              // Fallback: text without a text_start — lazily open a message item.
              const idx = nextOutputIndex++;
              const msgId = `msg_${randomUUID().replace(/-/g, "").slice(0, 24)}`;
              const fresh: OpenItem = { kind: "message", outputIndex: idx, itemId: msgId, text: "" };
              openItems.set(blockIdx, fresh);
              sse("response.output_item.added", {
                type: "response.output_item.added",
                output_index: idx,
                item: { id: msgId, type: "message", role: "assistant", status: "in_progress", content: [] },
              });
              sse("response.content_part.added", {
                type: "response.content_part.added",
                item_id: msgId,
                output_index: idx,
                content_index: 0,
                part: { type: "output_text", text: "" },
              });
              const delta = (msg.delta as string) ?? "";
              if (delta) {
                fresh.text += delta;
                sse("response.output_text.delta", {
                  type: "response.output_text.delta",
                  item_id: msgId,
                  output_index: idx,
                  content_index: 0,
                  delta,
                });
              }
              return;
            }
            const delta = (msg.delta as string) ?? "";
            if (!delta) return;
            open.text += delta;
            sse("response.output_text.delta", {
              type: "response.output_text.delta",
              item_id: open.itemId,
              output_index: open.outputIndex,
              content_index: 0,
              delta,
            });
            return;
          }
          if (chi === "chunk" && chunkType === "content_block_stop") {
            closeItem(blockIdx);
            return;
          }
          if (chi === "chunk" && chunkType === "tool_executed") {
            const callItemId = (msg.callId as string) ?? `mcp_${randomUUID().replace(/-/g, "").slice(0, 24)}`;
            const toolName = (msg.toolName as string) ?? "";
            const args = msg.arguments !== undefined
              ? (typeof msg.arguments === "string" ? msg.arguments : JSON.stringify(msg.arguments))
              : "{}";
            const output = (msg.output as string) ?? "";
            const isError = (msg.isError as boolean) === true;
            const serverLabel = toolName.includes("_") ? toolName.split("_", 2)[0] : "hum";
            const idx = nextOutputIndex++;
            const itemBase = {
              id: callItemId,
              type: "mcp_call",
              server_label: serverLabel,
              name: toolName,
              arguments: args,
              status: isError ? "failed" : "completed",
              output,
              ...(isError ? { error: { message: output } } : {}),
            };
            sse("response.output_item.added", {
              type: "response.output_item.added",
              output_index: idx,
              item: { ...itemBase, output: "", status: "in_progress" },
            });
            sse("response.output_item.done", {
              type: "response.output_item.done",
              output_index: idx,
              item: itemBase,
            });
            finalOutput.push(itemBase);
            return;
          }
          if (chi === "finish") {
            usage = msg.usage as typeof usage;
            // Close any items the worker left open (no explicit
            // content_block_stop for them).
            for (const idx of [...openItems.keys()]) closeItem(idx);
            const inputT = (usage?.input_tokens ?? 0)
              + (usage?.cache_read_input_tokens ?? 0)
              + (usage?.cache_creation_input_tokens ?? 0);
            const outputT = usage?.output_tokens ?? 0;
            const finalUsage = usage ? {
              input_tokens: inputT,
              output_tokens: outputT,
              total_tokens: inputT + outputT,
            } : undefined;
            if (finalUsage) trackUsage(tenant, finalUsage.input_tokens, finalUsage.output_tokens);
            const meta = sid_to_meta.get(sid);
            sse("response.completed", { type: "response.completed", response: {
              id: responseId,
              object: "response",
              created_at: createdAt,
              model,
              status: "completed",
              output: finalOutput,
              ...(meta?.metadata ? { metadata: meta.metadata } : {}),
              ...(meta?.safety_identifier ? { safety_identifier: meta.safety_identifier } : {}),
              ...(meta?.prompt_cache_key ? { prompt_cache_key: meta.prompt_cache_key } : {}),
              ...(finalUsage ? { usage: finalUsage } : {}),
            } });
            res.write("data: [DONE]\n\n");
            res.end();
            thrum.off(sid);
            return;
          }
          if (chi === "error") {
            sse("response.failed", { type: "response.failed", response: { id: responseId, object: "response", status: "failed", error: { message: (msg.message as string) ?? "unknown" } } });
            res.write("data: [DONE]\n\n");
            res.end();
            thrum.off(sid);
          }
        });
      } else {
        // Non-stream: accumulate all output items, return as one
        // Response body. Mirrors stream-path semantics: reasoning,
        // message, mcp_call all preserved in output[].
        let usage: { input_tokens?: number; output_tokens?: number; cache_read_input_tokens?: number; cache_creation_input_tokens?: number } | undefined;
        type NSOpen =
          | { kind: "reasoning"; itemId: string; text: string }
          | { kind: "message"; itemId: string; text: string };
        const nsOpen = new Map<number, NSOpen>();
        const nsOutput: Array<Record<string, unknown>> = [];
        const nsCloseItem = (blockIdx: number) => {
          const open = nsOpen.get(blockIdx);
          if (!open) return;
          nsOpen.delete(blockIdx);
          if (open.kind === "reasoning") {
            nsOutput.push({
              id: open.itemId,
              type: "reasoning",
              status: "completed",
              summary: [{ type: "summary_text", text: open.text }],
            });
          } else {
            nsOutput.push({
              id: open.itemId,
              type: "message",
              role: "assistant",
              status: "completed",
              content: [{ type: "output_text", text: open.text }],
            });
          }
        };
        thrum.on(sid, (msg) => {
          const chi = msg.chi as string | undefined;
          const chunkType = msg.chunkType as string | undefined;
          const blockIdx = typeof msg.id === "number" ? (msg.id as number)
            : typeof msg.blockIdx === "number" ? (msg.blockIdx as number)
            : -1;
          if (chi === "session-ready") {
            const nid = msg.nestId as string | undefined;
            if (nid) sid_to_nestid.set(sid, nid);
            return;
          }
          if (chi === "chunk" && chunkType === "reasoning_start") {
            nsOpen.set(blockIdx, { kind: "reasoning", itemId: `rs_${randomUUID().replace(/-/g, "").slice(0, 24)}`, text: "" });
            return;
          }
          if (chi === "chunk" && chunkType === "reasoning_delta") {
            const open = nsOpen.get(blockIdx);
            if (open?.kind === "reasoning") open.text += (msg.delta as string) ?? "";
            return;
          }
          if (chi === "chunk" && chunkType === "text_start") {
            nsOpen.set(blockIdx, { kind: "message", itemId: `msg_${randomUUID().replace(/-/g, "").slice(0, 24)}`, text: "" });
            return;
          }
          if (chi === "chunk" && chunkType === "text_delta") {
            let open = nsOpen.get(blockIdx);
            if (!open || open.kind !== "message") {
              open = { kind: "message", itemId: `msg_${randomUUID().replace(/-/g, "").slice(0, 24)}`, text: "" };
              nsOpen.set(blockIdx, open);
            }
            open.text += (msg.delta as string) ?? "";
            return;
          }
          if (chi === "chunk" && chunkType === "content_block_stop") {
            nsCloseItem(blockIdx);
            return;
          }
          if (chi === "chunk" && chunkType === "tool_executed") {
            const callItemId = (msg.callId as string) ?? `mcp_${randomUUID().replace(/-/g, "").slice(0, 24)}`;
            const toolName = (msg.toolName as string) ?? "";
            const args = msg.arguments !== undefined
              ? (typeof msg.arguments === "string" ? msg.arguments : JSON.stringify(msg.arguments))
              : "{}";
            const output = (msg.output as string) ?? "";
            const isError = (msg.isError as boolean) === true;
            const serverLabel = toolName.includes("_") ? toolName.split("_", 2)[0] : "hum";
            nsOutput.push({
              id: callItemId,
              type: "mcp_call",
              server_label: serverLabel,
              name: toolName,
              arguments: args,
              status: isError ? "failed" : "completed",
              output,
              ...(isError ? { error: { message: output } } : {}),
            });
            return;
          }
          if (chi === "finish") {
            usage = msg.usage as typeof usage;
            for (const idx of [...nsOpen.keys()]) nsCloseItem(idx);
            const inputT = (usage?.input_tokens ?? 0)
              + (usage?.cache_read_input_tokens ?? 0)
              + (usage?.cache_creation_input_tokens ?? 0);
            const outputT = usage?.output_tokens ?? 0;
            const finalUsage = usage ? {
              input_tokens: inputT,
              output_tokens: outputT,
              total_tokens: inputT + outputT,
            } : undefined;
            if (finalUsage) trackUsage(tenant, finalUsage.input_tokens, finalUsage.output_tokens);
            const meta = sid_to_meta.get(sid);
            res.writeHead(200, { "Content-Type": "application/json" });
            res.end(JSON.stringify({
              id: responseId,
              object: "response",
              created_at: createdAt,
              model,
              status: "completed",
              output: nsOutput,
              ...(meta?.metadata ? { metadata: meta.metadata } : {}),
              ...(meta?.safety_identifier ? { safety_identifier: meta.safety_identifier } : {}),
              ...(meta?.prompt_cache_key ? { prompt_cache_key: meta.prompt_cache_key } : {}),
              ...(finalUsage ? { usage: finalUsage } : {}),
            }));
            thrum.off(sid);
            return;
          }
          if (chi === "error") {
            res.writeHead(500, { "Content-Type": "application/json" });
            res.end(JSON.stringify({ error: { message: (msg.message as string) ?? "unknown", type: "internal_error" } }));
            thrum.off(sid);
          }
        });
      }

      req.on("close", () => { thrum.off(sid); });

      // Resume token: when we have a previously-seen claude session_id
      // for this sid, ship it so the worker spawns claude with
      // `--resume <id>` and the model rehydrates full prior context
      // (tool calls + results) from its session file. First turn:
      // no entry yet, claude spawns fresh and reports its
      // session_id back via chi:"session-ready" which we capture
      // for the next turn.
      const resume = sid_to_nestid.get(sid);
      thrum.send({
        chi: "prompt",
        sid,
        hive: "openai-server",
        modelId: model,
        content: userText,
        ...(systemPrompt ? { systemPrompt } : {}),
        ...(tools ? { tools } : {}),
        ...(Array.isArray(body.allowed_tools) && body.allowed_tools.length > 0 ? { allowedTools: body.allowed_tools } : {}),
        ...(respAttachments.length > 0 ? { attachments: respAttachments } : {}),
        ...(Object.keys(sampling).length > 0 ? { sampling } : {}),
        ...(resume ? { resume } : {}),
      });
      return;
    }

    // ── /v1/usage — read-only per-tenant usage ledger ─────────────────
    if (req.method === "GET" && url.pathname === "/v1/usage") {
      if (!checkAuth(req)) return unauthorized(res);
      const tenant = tenantOf(req);
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify({ tenant, usage: USAGE[tenant] ?? { promptTokens: 0, completionTokens: 0, totalTokens: 0, requests: 0 } }));
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

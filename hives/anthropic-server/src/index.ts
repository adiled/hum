// anthropic-server — Anthropic Messages API surface over hum.
//
// POST /v1/messages with `stream: true` translates to thrum's prompt /
// chunk / finish triad and pumps it back to the client as SSE events
// shaped like the official Anthropic API. Drop-in for the
// `@anthropic-ai/sdk` package — point baseURL at this server.
//
// Tool use is forwarded both ways: the model's tool_use blocks come
// out via chi:"tool-call", the client's tool_result content blocks
// come back via chi:"tool-result". The translation maps 1:1 because
// Anthropic's wire is already shaped like thrum's tool seam.

import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { randomUUID } from "node:crypto";
import { readFileSync, appendFileSync, mkdirSync, renameSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { ThrumClient } from "./thrum.ts";

interface NestlingConfig {
  host?: string;
  port?: number;
  apiKey?: string;
}

function readConfigFile(): NestlingConfig {
  const path = join(homedir(), ".config", "hum", "hives", "anthropic-server.json");
  try {
    const raw = readFileSync(path, "utf8");
    const parsed = JSON.parse(raw) as NestlingConfig;
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch {
    return {};
  }
}

const fileConfig = readConfigFile();

// Precedence: env > config file > built-in defaults.
const PORT = process.env.ANTHROPIC_SERVER_PORT !== undefined
  ? parseInt(process.env.ANTHROPIC_SERVER_PORT, 10)
  : (typeof fileConfig.port === "number" ? fileConfig.port : 14622);
const HOST = process.env.ANTHROPIC_SERVER_HOST
  ?? fileConfig.host
  ?? "127.0.0.1";
const API_KEY = process.env.ANTHROPIC_SERVER_API_KEY
  ?? fileConfig.apiKey
  ?? "";

// ── helpers ────────────────────────────────────────────────────────────────

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
  res.end(JSON.stringify({
    type: "error",
    error: { type: "authentication_error", message: "missing or invalid api key" },
  }));
}

function bad(res: ServerResponse, code: number, msg: string): void {
  res.writeHead(code, { "Content-Type": "application/json" });
  res.end(JSON.stringify({
    type: "error",
    error: { type: "invalid_request_error", message: msg },
  }));
}

function checkAuth(req: IncomingMessage): boolean {
  if (!API_KEY) return true; // unauthenticated mode for local dev
  // Anthropic SDK uses `x-api-key` header (not Bearer).
  const got = req.headers["x-api-key"] ?? req.headers["authorization"];
  if (typeof got !== "string") return false;
  return got === API_KEY || got === `Bearer ${API_KEY}`;
}

// ── tenant + audit + usage + rate-limit ────────────────────────────────
// Gateway concerns. Mirror openai-server's surface so operators get a
// uniform mental model across nestlings. Each nestling carries its own
// state directory to keep ledgers independent.

const STATE_DIR = (process.env.XDG_STATE_HOME ?? join(homedir(), ".local/state")) + "/hum/anthropic-server";
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
  inputTokens: number;
  outputTokens: number;
  totalTokens: number;
  requests: number;
}
const USAGE: Record<string, TenantUsage> = (() => {
  try { return JSON.parse(readFileSync(USAGE_PATH, "utf8")); } catch { return {}; }
})();
let usageDirty = false;
function trackUsage(tenant: string, input: number, output: number): void {
  const u = USAGE[tenant] ?? { inputTokens: 0, outputTokens: 0, totalTokens: 0, requests: 0 };
  u.inputTokens += input;
  u.outputTokens += output;
  u.totalTokens += input + output;
  u.requests += 1;
  USAGE[tenant] = u;
  usageDirty = true;
}
setInterval(() => {
  if (!usageDirty) return;
  try {
    appendFileSync(USAGE_PATH + ".tmp", JSON.stringify(USAGE, null, 2));
    renameSync(USAGE_PATH + ".tmp", USAGE_PATH);
    usageDirty = false;
  } catch {}
}, 30_000).unref();

interface Bucket { tokens: number; lastRefill: number; }
const BUCKETS: Record<string, Bucket> = {};
const RATE_CAPACITY = parseInt(process.env.ANTHROPIC_SERVER_RATE_CAPACITY ?? "60", 10);
const RATE_REFILL_PER_SEC = parseFloat(process.env.ANTHROPIC_SERVER_RATE_REFILL ?? "1.0");
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
  res.end(JSON.stringify({
    type: "error",
    error: { type: "rate_limit_error", message: `rate limit exceeded for tenant '${tenant}'` },
  }));
}

// ── Anthropic shapes (loose) ───────────────────────────────────────────────

interface ContentBlock {
  type: "text" | "tool_use" | "tool_result" | "image";
  text?: string;
  id?: string;
  name?: string;
  input?: unknown;
  tool_use_id?: string;
  content?: string | ContentBlock[];
}
interface AnthropicMessage {
  role: "user" | "assistant";
  content: string | ContentBlock[];
}
interface AnthropicTool {
  name: string;
  description?: string;
  input_schema: Record<string, unknown>;
}

function flattenContent(content: AnthropicMessage["content"]): string {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .filter((b) => b.type === "text")
    .map((b) => b.text ?? "")
    .join("\n");
}

function flattenSystem(system: unknown): string | undefined {
  if (typeof system === "string") return system;
  if (Array.isArray(system)) {
    return system
      .filter((p: { type?: string }) => p?.type === "text")
      .map((p: { text?: string }) => p.text ?? "")
      .join("\n\n") || undefined;
  }
  return undefined;
}

function lastUserMessage(messages: AnthropicMessage[]): string {
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i].role === "user") return flattenContent(messages[i].content);
  }
  return "";
}

function trailingToolResults(messages: AnthropicMessage[]): Array<{ tool_use_id: string; result: string }> {
  // The last user message may carry tool_result blocks answering the
  // previous assistant turn's tool_use blocks. Pull those out.
  if (messages.length === 0) return [];
  const last = messages[messages.length - 1];
  if (last.role !== "user" || typeof last.content === "string") return [];
  const out: Array<{ tool_use_id: string; result: string }> = [];
  for (const block of last.content) {
    if (block.type !== "tool_result" || !block.tool_use_id) continue;
    const result = typeof block.content === "string"
      ? block.content
      : Array.isArray(block.content)
        ? block.content.filter((b) => b.type === "text").map((b) => b.text ?? "").join("\n")
        : "";
    out.push({ tool_use_id: block.tool_use_id, result });
  }
  return out;
}

function toolsToThrum(tools: AnthropicTool[] | undefined) {
  if (!Array.isArray(tools) || tools.length === 0) return undefined;
  return tools.map((t) => ({
    name: t.name,
    ...(t.description ? { description: t.description } : {}),
    ...(t.input_schema ? { parameters: t.input_schema } : {}),
  }));
}

// ── SSE writer ─────────────────────────────────────────────────────────────

function writeSseHeaders(res: ServerResponse): void {
  res.writeHead(200, {
    "Content-Type": "text/event-stream",
    "Cache-Control": "no-cache",
    "Connection": "keep-alive",
    "X-Accel-Buffering": "no",
  });
}

function sse(res: ServerResponse, event: string, data: unknown): void {
  res.write(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`);
}

// ── /v1/messages ───────────────────────────────────────────────────────────

interface MessagesRequest {
  model: string;
  messages: AnthropicMessage[];
  system?: unknown;
  tools?: AnthropicTool[];
  stream?: boolean;
  max_tokens?: number;
}

async function handleMessages(req: IncomingMessage, res: ServerResponse, tenant: string): Promise<void> {
  const raw = await readBody(req);
  let body: MessagesRequest;
  try {
    body = JSON.parse(raw) as MessagesRequest;
  } catch {
    return bad(res, 400, "invalid json body");
  }
  if (!body.model) return bad(res, 400, "model required");
  if (!Array.isArray(body.messages)) return bad(res, 400, "messages required");

  // Tenant-prefixed sid keeps multi-tenant traffic isolated on the wire.
  const baseSid = randomUUID();
  const sid = tenant === "default" ? baseSid : `${tenant}:${baseSid}`;
  const messageId = `msg_${randomUUID().replace(/-/g, "")}`;
  const stream = body.stream !== false;
  const tools = toolsToThrum(body.tools);
  const systemPrompt = flattenSystem(body.system);
  const text = lastUserMessage(body.messages);
  const toolReturns = trailingToolResults(body.messages);
  audit({ endpoint: "messages", tenant, model: body.model, sid, stream });

  const client = new ThrumClient();
  await client.connect();

  // Open a fresh prompt OR continue with tool results. Anthropic
  // convention-stateful: each request is a fresh sid; tool_result
  // blocks in the messages array continue the prior tool_use.
  if (toolReturns.length > 0) {
    for (const ret of toolReturns) {
      client.send({
        chi: "tool-result",
        rid: `tr-${ret.tool_use_id}`,
        sid,
        callId: ret.tool_use_id,
        result: { content: ret.result },
      });
    }
  }
  client.send({
    chi: "prompt",
    rid: `prompt-${sid}`,
    sid,
    text,
    modelId: body.model,
    ...(systemPrompt ? { systemPrompt } : {}),
    ...(tools ? { tools } : {}),
  });

  if (!stream) {
    return handleNonStream(client, res, sid, messageId, body.model, tenant);
  }
  return handleStream(client, res, sid, messageId, body.model, tenant);
}

async function handleNonStream(
  client: ThrumClient,
  res: ServerResponse,
  sid: string,
  messageId: string,
  model: string,
  tenant: string,
): Promise<void> {
  const text: string[] = [];
  const toolUses: ContentBlock[] = [];
  let stopReason = "end_turn";
  let usage: Record<string, number> = { input_tokens: 0, output_tokens: 0 };

  await new Promise<void>((resolve) => {
    client.on(sid, (tone) => {
      const chi = tone.chi as string;
      if (chi === "chunk") {
        const part = tone.part as { type?: string; text?: string; toolCall?: ContentBlock } | undefined;
        if (part?.type === "text" && typeof part.text === "string") text.push(part.text);
        else if (part?.type === "tool_use" && part.toolCall) toolUses.push(part.toolCall);
      } else if (chi === "finish") {
        stopReason = (tone.finishReason as string) || "end_turn";
        if (tone.usage && typeof tone.usage === "object") usage = { ...usage, ...(tone.usage as Record<string, number>) };
        client.off(sid);
        resolve();
      } else if (chi === "error") {
        stopReason = "error";
        client.off(sid);
        resolve();
      }
    });
  });

  const content: ContentBlock[] = [];
  if (text.length > 0) content.push({ type: "text", text: text.join("") });
  for (const tu of toolUses) content.push(tu);

  trackUsage(tenant, usage.input_tokens ?? 0, usage.output_tokens ?? 0);

  res.writeHead(200, { "Content-Type": "application/json" });
  res.end(JSON.stringify({
    id: messageId,
    type: "message",
    role: "assistant",
    model,
    content,
    stop_reason: stopReason,
    usage,
  }));
}

async function handleStream(
  client: ThrumClient,
  res: ServerResponse,
  sid: string,
  messageId: string,
  model: string,
  tenant: string,
): Promise<void> {
  writeSseHeaders(res);

  sse(res, "message_start", {
    type: "message_start",
    message: { id: messageId, type: "message", role: "assistant", model, content: [], stop_reason: null, usage: { input_tokens: 0, output_tokens: 0 } },
  });

  let textIndex = -1;
  const openToolBlocks = new Map<string, number>();
  let nextIndex = 0;

  return new Promise<void>((resolve) => {
    client.on(sid, (tone) => {
      const chi = tone.chi as string;
      if (chi === "chunk") {
        const part = tone.part as { type?: string; text?: string; toolCall?: { id?: string; name?: string; input?: unknown; partial?: string } } | undefined;
        if (part?.type === "text" && typeof part.text === "string") {
          if (textIndex < 0) {
            textIndex = nextIndex++;
            sse(res, "content_block_start", { type: "content_block_start", index: textIndex, content_block: { type: "text", text: "" } });
          }
          sse(res, "content_block_delta", { type: "content_block_delta", index: textIndex, delta: { type: "text_delta", text: part.text } });
        } else if (part?.type === "tool_use" && part.toolCall?.id) {
          const idx = openToolBlocks.get(part.toolCall.id);
          if (idx === undefined) {
            const newIdx = nextIndex++;
            openToolBlocks.set(part.toolCall.id, newIdx);
            sse(res, "content_block_start", {
              type: "content_block_start",
              index: newIdx,
              content_block: {
                type: "tool_use",
                id: part.toolCall.id,
                name: part.toolCall.name ?? "",
                input: {},
              },
            });
            if (typeof part.toolCall.partial === "string") {
              sse(res, "content_block_delta", {
                type: "content_block_delta",
                index: newIdx,
                delta: { type: "input_json_delta", partial_json: part.toolCall.partial },
              });
            }
          } else if (typeof part.toolCall.partial === "string") {
            sse(res, "content_block_delta", {
              type: "content_block_delta",
              index: idx,
              delta: { type: "input_json_delta", partial_json: part.toolCall.partial },
            });
          }
        }
      } else if (chi === "finish") {
        if (textIndex >= 0) sse(res, "content_block_stop", { type: "content_block_stop", index: textIndex });
        for (const idx of openToolBlocks.values()) {
          sse(res, "content_block_stop", { type: "content_block_stop", index: idx });
        }
        const finishUsage = (tone.usage as Record<string, number>) ?? { output_tokens: 0 };
        trackUsage(tenant, finishUsage.input_tokens ?? 0, finishUsage.output_tokens ?? 0);
        sse(res, "message_delta", {
          type: "message_delta",
          delta: { stop_reason: (tone.finishReason as string) || "end_turn", stop_sequence: null },
          usage: finishUsage,
        });
        sse(res, "message_stop", { type: "message_stop" });
        client.off(sid);
        res.end();
        resolve();
      } else if (chi === "error") {
        const msg = (tone.message as string) ?? "stream error";
        sse(res, "error", { type: "error", error: { type: "api_error", message: msg } });
        client.off(sid);
        res.end();
        resolve();
      }
    });
  });
}

// ── server ─────────────────────────────────────────────────────────────────

const server = createServer(async (req, res) => {
  if (!checkAuth(req)) return unauthorized(res);
  if (req.method === "POST" && req.url === "/v1/messages") {
    const tenant = tenantOf(req);
    if (!allow(tenant)) return tooManyRequests(res, tenant);
    try {
      await handleMessages(req, res, tenant);
    } catch (e) {
      bad(res, 500, (e as Error).message || "internal error");
    }
    return;
  }
  if (req.method === "GET" && req.url === "/v1/usage") {
    const tenant = tenantOf(req);
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ tenant, usage: USAGE[tenant] ?? { inputTokens: 0, outputTokens: 0, totalTokens: 0, requests: 0 } }));
    return;
  }
  if (req.method === "GET" && req.url === "/") {
    res.writeHead(200, { "Content-Type": "text/plain" });
    res.end("hum-anthropic-server\n");
    return;
  }
  res.writeHead(404, { "Content-Type": "application/json" });
  res.end(JSON.stringify({ type: "error", error: { type: "not_found_error", message: "not found" } }));
});

async function start(): Promise<void> {
  await new Promise<void>((resolve) => {
    server.listen(PORT, HOST, () => resolve());
  });

  const addr = server.address();
  const boundHost = typeof addr === "object" && addr !== null ? addr.address : HOST;
  const boundPort = typeof addr === "object" && addr !== null ? addr.port : PORT;

  // Persistent startup-level client announces the nestling with its
  // actual bound address. Per-request handlers use their own short-
  // lived clients.
  const registrar = new ThrumClient();
  try {
    await registrar.connect({ host: boundHost, port: boundPort, scheme: "http" });
  } catch (e) {
    console.warn(`anthropic-server: thrum announce failed: ${(e as Error).message}`);
  }

  console.log(`anthropic-server listening on http://${boundHost}:${boundPort}`);
}

start().catch((e) => { console.error("anthropic-server: startup failed:", e); process.exit(1); });

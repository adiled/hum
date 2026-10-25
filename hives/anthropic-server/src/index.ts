// anthropic-server — Anthropic Messages API surface over hum.
//
// POST /v1/messages becomes a prompt, and the bloom comes back to the
// client as SSE events shaped like the official Anthropic API: a text
// petal is a text block and a sepal a thinking block, signed by its
// propolis. Drop-in for the `@anthropic-ai/sdk` package — point baseURL
// at this server.
//
// Tool use is forwarded both ways: a tendril that reaches is a tool_use
// block that ends the message with stop_reason `tool_use`, and the
// client's tool_result blocks in its next message come back as
// chi:"tendril-result". Anthropic's wire is already shaped like thrum's
// tendrils, so the translation maps 1:1. Each request is a fresh sid,
// and its tool_result blocks continue the tool_use they answer.
//
// The hive is one bee, a forager. It announces where its HTTP surface
// listens, and every request prompts through it under a sigil of its
// own, tenant-prefixed so tenants stay apart on the wire. When humd goes
// away mid-bloom, every open sigil hears an error and its response ends;
// a request that arrives while humd is away is turned away.
//
// Configuration comes from the environment, then
// ~/.config/hum/hives/anthropic-server.json, then the defaults. With no
// API key the server is open, for local development; the Anthropic SDK
// sends its key as `x-api-key`, and a Bearer token works too. The
// gateway concerns (tenant, audit, usage, rate limit) mirror
// openai-server's, so operators get one model across bees, and each bee
// keeps its own ledgers in its own state directory.

import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { randomUUID } from "node:crypto";
import { readFileSync, appendFileSync, mkdirSync, renameSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { Bee, PetalKind, type Hello, type Tone } from "../../../thrum-clients/ts/index.ts";
import pkg from "../package.json";

const HIVE_NAME = "anthropic-server";

type SidHandler = (tone: Tone) => void;

const hello: Hello = {
  roles: ["forager"],
  version: pkg.version,
  propensity: { statefulness: "convention-stateful", richness: "medium", wire: "anthropic/messages" },
  chis: ["hello", "prompt", "cancel", "unfurl", "wilt", "error", "tendril-reach", "tendril-result"],
  source: "https://github.com/adiled/hum/tree/main/hives/anthropic-server",
};
const bee = new Bee({ hive: HIVE_NAME, as: "main", hello });
const sigils = new Map<string, SidHandler>();
let nestled = false;
bee.onNestled(() => {
  nestled = true;
});
bee.onAstray(() => {
  nestled = false;
  for (const [sid, fn] of [...sigils]) fn({ chi: "error", sid, message: "humd went away mid-bloom" });
});

const client = {
  on(sid: string, fn: SidHandler): void {
    sigils.set(sid, fn);
    bee.onSid(sid, fn);
  },
  off(sid: string): void {
    sigils.delete(sid);
    bee.offSid(sid);
  },
  send(tone: Tone): void {
    bee.send(tone);
  },
};
type Client = typeof client;

interface HiveConfig {
  host?: string;
  port?: number;
  apiKey?: string;
}

function readConfigFile(): HiveConfig {
  const path = join(homedir(), ".config", "hum", "hives", "anthropic-server.json");
  try {
    const raw = readFileSync(path, "utf8");
    const parsed = JSON.parse(raw) as HiveConfig;
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch {
    return {};
  }
}

const fileConfig = readConfigFile();

const PORT = process.env.ANTHROPIC_SERVER_PORT !== undefined
  ? parseInt(process.env.ANTHROPIC_SERVER_PORT, 10)
  : (typeof fileConfig.port === "number" ? fileConfig.port : 14622);
const HOST = process.env.ANTHROPIC_SERVER_HOST
  ?? fileConfig.host
  ?? "127.0.0.1";
const API_KEY = process.env.ANTHROPIC_SERVER_API_KEY
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
  if (!API_KEY) return true;
  const got = req.headers["x-api-key"] ?? req.headers["authorization"];
  if (typeof got !== "string") return false;
  return got === API_KEY || got === `Bearer ${API_KEY}`;
}

const STATE_DIR = (process.env.XDG_STATE_HOME ?? join(homedir(), ".local/state")) + "/hum/hives/anthropic-server";
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

interface ContentBlock {
  type: "text" | "thinking" | "tool_use" | "tool_result" | "image";
  text?: string;
  thinking?: string;
  signature?: string;
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
    inputSchema: t.input_schema ?? {},
  }));
}

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

  const baseSid = randomUUID();
  const sid = tenant === "default" ? baseSid : `${tenant}:${baseSid}`;
  const messageId = `msg_${randomUUID().replace(/-/g, "")}`;
  const stream = body.stream !== false;
  const tools = toolsToThrum(body.tools);
  const systemPrompt = flattenSystem(body.system);
  const text = lastUserMessage(body.messages);
  const toolReturns = trailingToolResults(body.messages);
  audit({ endpoint: "messages", tenant, model: body.model, sid, stream });

  if (!nestled) return bad(res, 503, "humd is away; try again shortly");

  if (toolReturns.length > 0) {
    for (const ret of toolReturns) {
      client.send({
        chi: "tendril-result",
        rid: `tr-${ret.tool_use_id}`,
        sid,
        callId: ret.tool_use_id,
        result: ret.result,
      });
    }
  }
  client.send({
    chi: "prompt",
    rid: `prompt-${sid}`,
    sid,
    content: [{ kind: PetalKind.text, text }],
    modelId: body.model,
    ...(typeof body.max_tokens === "number" ? { season: body.max_tokens } : {}),
    ...(systemPrompt ? { systemPrompt } : {}),
    ...(tools ? { tools } : {}),
  });

  if (!stream) {
    return handleNonStream(client, res, sid, messageId, body.model, tenant);
  }
  return handleStream(client, res, sid, messageId, body.model, tenant);
}

function stopReasonOf(reason: string | undefined): string {
  if (!reason || reason === "stop" || reason === "success") return "end_turn";
  if (reason === "length") return "max_tokens";
  return reason;
}

function nectarOf(tone: Tone<"wilt">): Record<string, number> {
  const nectar = tone.nectar;
  return nectar && typeof nectar === "object" ? (nectar as Record<string, number>) : {};
}

async function handleNonStream(
  client: Client,
  res: ServerResponse,
  sid: string,
  messageId: string,
  model: string,
  tenant: string,
): Promise<void> {
  const blocks = new Map<number, ContentBlock>();
  const toolUses: ContentBlock[] = [];
  let stopReason = "end_turn";
  let usage: Record<string, number> = { input_tokens: 0, output_tokens: 0 };

  await new Promise<void>((resolve) => {
    client.on(sid, (tone) => {
      if (tone.chi === "unfurl") {
        let block = blocks.get(tone.petal);
        if (!block && tone.kind === PetalKind.text) blocks.set(tone.petal, (block = { type: "text", text: "" }));
        if (!block && tone.kind === PetalKind.sepal) blocks.set(tone.petal, (block = { type: "thinking", thinking: "" }));
        if (block?.type === "text" && tone.text) block.text += tone.text;
        if (block?.type === "thinking" && tone.text) block.thinking += tone.text;
        if (block?.type === "thinking" && tone.propolis) block.signature = tone.propolis;
      } else if (tone.chi === "tendril-reach") {
        toolUses.push({ type: "tool_use", id: tone.callId, name: tone.name, input: tone.args ?? {} });
        stopReason = "tool_use";
        client.off(sid);
        resolve();
      } else if (tone.chi === "wilt") {
        stopReason = stopReasonOf(tone.reason);
        usage = { ...usage, ...nectarOf(tone) };
        client.off(sid);
        resolve();
      } else if (tone.chi === "error") {
        stopReason = "error";
        client.off(sid);
        resolve();
      }
    });
  });

  const content: ContentBlock[] = [...blocks.entries()].sort(([a], [b]) => a - b).map(([, block]) => block);
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
  client: Client,
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

  const open = new Map<number, { index: number; kind: string }>();
  let nextIndex = 0;

  const blockFor = (petal: number, kind: string): { index: number; kind: string } | undefined => {
    const known = open.get(petal);
    if (known) return known;
    if (kind !== PetalKind.text && kind !== PetalKind.sepal) return undefined;
    const fresh = { index: nextIndex++, kind };
    open.set(petal, fresh);
    const contentBlock = kind === PetalKind.text ? { type: "text", text: "" } : { type: "thinking", thinking: "" };
    sse(res, "content_block_start", { type: "content_block_start", index: fresh.index, content_block: contentBlock });
    return fresh;
  };
  const closeAll = () => {
    for (const { index } of open.values()) sse(res, "content_block_stop", { type: "content_block_stop", index });
    open.clear();
  };
  const end = (stopReason: string, usage: Record<string, number>) => {
    trackUsage(tenant, usage.input_tokens ?? 0, usage.output_tokens ?? 0);
    sse(res, "message_delta", { type: "message_delta", delta: { stop_reason: stopReason, stop_sequence: null }, usage });
    sse(res, "message_stop", { type: "message_stop" });
    client.off(sid);
    res.end();
  };

  return new Promise<void>((resolve) => {
    client.on(sid, (tone) => {
      if (tone.chi === "unfurl") {
        if (tone.closed) {
          const block = open.get(tone.petal);
          if (block) {
            sse(res, "content_block_stop", { type: "content_block_stop", index: block.index });
            open.delete(tone.petal);
          }
          return;
        }
        const block = blockFor(tone.petal, tone.kind);
        if (!block) return;
        if (block.kind === PetalKind.text && tone.text) {
          sse(res, "content_block_delta", { type: "content_block_delta", index: block.index, delta: { type: "text_delta", text: tone.text } });
        }
        if (block.kind === PetalKind.sepal && tone.text) {
          sse(res, "content_block_delta", { type: "content_block_delta", index: block.index, delta: { type: "thinking_delta", thinking: tone.text } });
        }
        if (block.kind === PetalKind.sepal && tone.propolis) {
          sse(res, "content_block_delta", { type: "content_block_delta", index: block.index, delta: { type: "signature_delta", signature: tone.propolis } });
        }
      } else if (tone.chi === "tendril-reach") {
        closeAll();
        const index = nextIndex++;
        sse(res, "content_block_start", {
          type: "content_block_start",
          index,
          content_block: { type: "tool_use", id: tone.callId, name: tone.name, input: {} },
        });
        sse(res, "content_block_delta", {
          type: "content_block_delta",
          index,
          delta: { type: "input_json_delta", partial_json: JSON.stringify(tone.args ?? {}) },
        });
        sse(res, "content_block_stop", { type: "content_block_stop", index });
        end("tool_use", { output_tokens: 0 });
        resolve();
      } else if (tone.chi === "wilt") {
        closeAll();
        end(stopReasonOf(tone.reason), { output_tokens: 0, ...nectarOf(tone) });
        resolve();
      } else if (tone.chi === "error") {
        sse(res, "error", { type: "error", error: { type: "api_error", message: tone.message ?? "stream error" } });
        client.off(sid);
        res.end();
        resolve();
      }
    });
  });
}

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

  hello.bind = { host: boundHost, port: boundPort, scheme: "http" };
  await bee.start();
  bee.closed().catch((e) => {
    console.error("anthropic-server: the bee ended:", e);
    process.exit(1);
  });

  console.log(`anthropic-server listening on http://${boundHost}:${boundPort}`);
}

start().catch((e) => { console.error("anthropic-server: startup failed:", e); process.exit(1); });

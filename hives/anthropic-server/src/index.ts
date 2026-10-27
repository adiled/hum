// anthropic-server: Anthropic's Messages API in front of hum.
//
//   POST /v1/messages     one bloom, as one message or streamed as Anthropic's events (messages.ts)
//   GET  /v1/usage        what the caller's tenant has taken
//   GET  /                hum-anthropic-server
//
// A route is its path; a query, such as the SDK's `?beta=true`, is not
// read. Drop-in for `@anthropic-ai/sdk`: point its baseURL here.
//
// The hive is one bee, a forager (forager.ts), and every request prompts
// through it on a sigil of its own, tenant-prefixed so tenants stay apart
// on the wire. A request that arrives while humd is away is turned away.
//
// Configuration comes from the environment, then the hive's config,
// `hives/anthropic-server.json` under hum's config root, then the
// defaults. With no API key the server is open, for local development;
// the SDK sends its key as `x-api-key`, and a Bearer token works too. The
// gateway concerns (tenant, audit, usage, rate limit) mirror
// openai-server's, and the hive keeps its ledgers in its own state.

import { createServer } from "node:http";
import { appendFileSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { Sigils } from "./conversation.ts";
import { Forager, HIVE } from "./forager.ts";
import type { Usage } from "./growth.ts";
import { HttpError, sendError, sendJson } from "./http.ts";
import { messages, type Ctx } from "./messages.ts";

function xdg(name: string, fallback: string): string {
  const set = process.env[name];
  return set && set.length > 0 ? set : join(homedir(), fallback);
}

const file: Record<string, any> = (() => {
  try {
    const parsed = JSON.parse(readFileSync(join(xdg("XDG_CONFIG_HOME", ".config"), "hum", "hives", `${HIVE}.json`), "utf8"));
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch {
    return {};
  }
})();

function num(v: unknown): number | undefined {
  const n = typeof v === "string" ? Number(v) : v;
  return typeof n === "number" && Number.isFinite(n) ? n : undefined;
}

const env = process.env;
const PORT = num(env.ANTHROPIC_SERVER_PORT) ?? num(file.port) ?? 14622;
const HOST = env.ANTHROPIC_SERVER_HOST ?? (typeof file.host === "string" ? file.host : "127.0.0.1");
const API_KEY = env.ANTHROPIC_SERVER_API_KEY ?? (typeof file.apiKey === "string" ? file.apiKey : "");
const RATE_CAPACITY = num(env.ANTHROPIC_SERVER_RATE_CAPACITY) ?? num(file.rate?.capacity) ?? 60;
const RATE_REFILL_PER_SEC = num(env.ANTHROPIC_SERVER_RATE_REFILL) ?? num(file.rate?.refillPerSec) ?? 1;

const STATE_DIR = join(xdg("XDG_STATE_HOME", ".local/state"), "hum", "hives", HIVE);
const AUDIT_LOG = join(STATE_DIR, "audit.log");
const USAGE_PATH = join(STATE_DIR, "usage.json");
try { mkdirSync(STATE_DIR, { recursive: true }); } catch {}

function checkAuth(headers: NodeJS.Dict<string | string[]>): boolean {
  if (!API_KEY) return true;
  const got = headers["x-api-key"] ?? headers["authorization"];
  return got === API_KEY || got === `Bearer ${API_KEY}`;
}

function tenantOf(headers: NodeJS.Dict<string | string[]>): string {
  const h = headers["x-tenant"];
  const t = typeof h === "string" ? h.replace(/[^A-Za-z0-9_-]/g, "") : "";
  return t || "default";
}

function audit(entry: Record<string, unknown>): void {
  try { appendFileSync(AUDIT_LOG, JSON.stringify({ ts: new Date().toISOString(), ...entry }) + "\n"); } catch {}
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
function track(tenant: string, used: Usage): void {
  const u = USAGE[tenant] ?? { inputTokens: 0, outputTokens: 0, totalTokens: 0, requests: 0 };
  u.inputTokens += used.input_tokens;
  u.outputTokens += used.output_tokens;
  u.totalTokens += used.input_tokens + used.output_tokens;
  u.requests += 1;
  USAGE[tenant] = u;
  usageDirty = true;
}
setInterval(() => {
  if (!usageDirty) return;
  try {
    writeFileSync(USAGE_PATH + ".tmp", JSON.stringify(USAGE, null, 2));
    renameSync(USAGE_PATH + ".tmp", USAGE_PATH);
    usageDirty = false;
  } catch {}
}, 30_000).unref();

const BUCKETS = new Map<string, { tokens: number; at: number }>();
function take(tenant: string): void {
  const now = Date.now();
  const b = BUCKETS.get(tenant) ?? { tokens: RATE_CAPACITY, at: now };
  b.tokens = Math.min(RATE_CAPACITY, b.tokens + ((now - b.at) / 1000) * RATE_REFILL_PER_SEC);
  b.at = now;
  BUCKETS.set(tenant, b);
  if (b.tokens < 1) throw new HttpError(429, `rate limit exceeded for tenant '${tenant}'`);
  b.tokens -= 1;
}

const forager = new Forager();
const ctx: Ctx = {
  forager,
  sigils: new Sigils((sid) => forager.forget(sid)),
  audit,
  track,
};

const server = createServer((req, res) => {
  const route = async () => {
    if (!checkAuth(req.headers)) throw new HttpError(401, "missing or invalid api key");
    const path = new URL(req.url ?? "/", "http://hum").pathname;
    const tenant = tenantOf(req.headers);
    if (req.method === "POST" && path === "/v1/messages") {
      try {
        take(tenant);
      } catch (e) {
        sendError(res, e, { "Retry-After": "60", "X-RateLimit-Tenant": tenant });
        return;
      }
      return messages(ctx, tenant, req, res);
    }
    if (req.method === "GET" && path === "/v1/usage") {
      return sendJson(res, 200, { tenant, usage: USAGE[tenant] ?? { inputTokens: 0, outputTokens: 0, totalTokens: 0, requests: 0 } });
    }
    if (req.method === "GET" && path === "/") {
      res.writeHead(200, { "Content-Type": "text/plain" });
      res.end("hum-anthropic-server\n");
      return;
    }
    throw new HttpError(404, `${req.method} ${path} is not served here`);
  };
  route().catch((e) => {
    if (!res.headersSent) sendError(res, e);
    else if (!res.writableEnded) res.end();
  });
});

async function start(): Promise<void> {
  await new Promise<void>((resolve) => server.listen(PORT, HOST, resolve));
  const addr = server.address();
  const host = addr && typeof addr === "object" ? addr.address : HOST;
  const port = addr && typeof addr === "object" ? addr.port : PORT;
  forager.hello.bind = { host, port, scheme: "http" };
  await forager.bee.start();
  forager.bee.closed().catch((e) => {
    console.error("anthropic-server: the bee ended:", e);
    process.exit(1);
  });
  console.log(`anthropic-server listening on http://${host}:${port}`);
}

start().catch((e) => {
  console.error("anthropic-server: startup failed:", e);
  process.exit(1);
});

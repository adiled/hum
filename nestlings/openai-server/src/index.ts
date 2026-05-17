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

    if (req.method === "POST" && url.pathname === "/v1/chat/completions") {
      if (!checkAuth(req)) return unauthorized(res);
      let body: { messages?: OpenAIMessage[]; model?: string; stream?: boolean; user?: string; tools?: OpenAITool[] };
      try { body = JSON.parse(await readBody(req)); } catch { return bad(res, "invalid JSON body"); }
      const messages = body.messages ?? [];
      if (messages.length === 0) return bad(res, "messages required");

      const stream = body.stream !== false; // default to streaming
      // body.model is the only correct source — the client picks. If
      // absent, fall back to the first advertised id; if none, use a
      // pass-through tag humd will reject loudly rather than guess.
      const model = body.model ?? MODEL_IDS[0] ?? "unspecified";
      // body.user wins when the client supplies a session id; otherwise
      // derive a stable one from the conversation anchor.
      const sid = body.user ?? `oai-${sessionKey(messages)}`;
      const { systemPrompt, userPrompt } = messagesToPrompt(messages);
      const tools = toolsFromOpenAI(body.tools);

      if (!stream) {
        return bad(res, "non-streaming not implemented; pass stream:true");
      }

      res.writeHead(200, {
        "Content-Type": "text/event-stream",
        "Cache-Control": "no-cache",
        "Connection": "keep-alive",
        "X-Accel-Buffering": "no",
      });

      const translator = new OpenAITranslator(`chatcmpl-${randomUUID()}`, model);

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

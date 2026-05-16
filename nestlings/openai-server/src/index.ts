import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { randomUUID } from "node:crypto";
import { ThrumClient } from "./thrum.ts";
import { OpenAITranslator } from "./transform.ts";

const PORT = parseInt(process.env.HUM_OPENAI_PORT ?? "14620", 10);
const HOST = process.env.HUM_OPENAI_HOST ?? "127.0.0.1";
const API_KEY = process.env.HUM_OPENAI_API_KEY ?? "";

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

function messagesToPrompt(messages: OpenAIMessage[]): { systemPrompt?: string; userPrompt: string } {
  const systemPieces: string[] = [];
  let userPrompt = "";
  for (const msg of messages) {
    if (msg.role === "system") systemPieces.push(flatten(msg.content));
    else if (msg.role === "user") userPrompt = flatten(msg.content); // last user wins
  }
  return {
    systemPrompt: systemPieces.length > 0 ? systemPieces.join("\n\n") : undefined,
    userPrompt,
  };
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
  await thrum.connect();
  console.log(`[hum-openai-server] connected to thrum`);

  const server = createServer(async (req, res) => {
    const url = new URL(req.url ?? "/", `http://${req.headers.host ?? "localhost"}`);

    if (req.method === "GET" && url.pathname === "/v1/models") {
      if (!checkAuth(req)) return unauthorized(res);
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify({
        object: "list",
        data: [
          { id: "claude-opus-4-7",    object: "model", created: 0, owned_by: "hum" },
          { id: "claude-sonnet-4-6",  object: "model", created: 0, owned_by: "hum" },
          { id: "claude-haiku-4-5",   object: "model", created: 0, owned_by: "hum" },
        ],
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
      const model = body.model ?? "claude-sonnet-4-6";
      const sid = body.user ?? `oai-${randomUUID()}`;
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
          nest: "claude-cli",
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

  server.listen(PORT, HOST, () => {
    console.log(`[hum-openai-server] listening on http://${HOST}:${PORT}`);
  });
}

start().catch(e => { console.error("[hum-openai-server] startup failed:", e); process.exit(1); });

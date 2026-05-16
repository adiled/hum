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

function messagesToPrompt(messages: OpenAIMessage[]): { systemPrompt?: string; userPrompt: string } {
  const systemPieces: string[] = [];
  const conversation: string[] = [];
  for (const msg of messages) {
    const text = typeof msg.content === "string"
      ? msg.content
      : Array.isArray(msg.content)
        ? msg.content.filter(p => p.type === "text").map(p => p.text ?? "").join("\n")
        : "";
    if (msg.role === "system") systemPieces.push(text);
    else if (msg.role === "user") conversation.push(text);
    else if (msg.role === "assistant") conversation.push(text);
  }
  return {
    systemPrompt: systemPieces.length > 0 ? systemPieces.join("\n\n") : undefined,
    userPrompt: conversation[conversation.length - 1] ?? "",
  };
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
      let body: { messages?: OpenAIMessage[]; model?: string; stream?: boolean; user?: string };
      try { body = JSON.parse(await readBody(req)); } catch { return bad(res, "invalid JSON body"); }
      const messages = body.messages ?? [];
      if (messages.length === 0) return bad(res, "messages required");

      const stream = body.stream !== false; // default to streaming
      const model = body.model ?? "claude-sonnet-4-6";
      const sid = body.user ?? `oai-${randomUUID()}`;
      const { systemPrompt, userPrompt } = messagesToPrompt(messages);

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
        thrum.send({ chi: "cancel", sid, reason: "client-disconnect" });
      });

      thrum.send({
        chi: "prompt",
        sid,
        modelId: model,
        content: userPrompt,
        ...(systemPrompt ? { systemPrompt } : {}),
        cwd: process.env.HOME ?? "/",
      });
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

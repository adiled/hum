// A scripted OpenAI-compatible API, standing in for the one upstream. It
// runs under Node in a process of its own, as a real API would, and says
// what it heard at /_heard.
//
// It lists two chat models and an embedding model, records every request
// it is sent, and answers by what the last user turn says: ONE calls
// readFile once, PAR calls it twice at once, LOOKUP calls lookup, NOTFOUND
// calls it for nothing, which it fails to find, a question about the
// weather calls the weather tool it was offered, SLOW streams twenty ticks
// slowly, LEN stops for length, FAIL answers 500. Anything else is echoed
// back, or, after tool answers, the answers are. It refuses a conversation
// with a tool call nobody answered, as OpenAI does. /embeddings scores
// each text by its length and its vowels, and /completions completes a
// prompt with a marker and its suffix.

import { spawn } from "node:child_process";
import { createServer, type Server } from "node:http";

export interface Heard {
  url: string;
  body: any;
}

export interface Upstream {
  url: string;
  log(): Promise<{ heard: Heard[]; aborted: string[] }>;
  close(): Promise<void>;
}

const mk = (o: unknown) => `data: ${JSON.stringify(o)}\n\n`;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function lastUser(messages: any[]): string {
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i].role === "user") return typeof messages[i].content === "string" ? messages[i].content : JSON.stringify(messages[i].content);
  }
  return "";
}

export async function serve(at = 0): Promise<{ url: string; heard: Heard[]; aborted: string[]; close: () => Promise<void> }> {
  const heard: Heard[] = [];
  const aborted: string[] = [];
  const server: Server = createServer((req, res) => {
    let raw = "";
    req.on("data", (c) => (raw += c));
    req.on("end", async () => {
      if (req.method === "GET" && req.url === "/_heard") {
        res.writeHead(200, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ heard, aborted }));
        return;
      }
      if (req.method === "GET" && req.url?.endsWith("/models")) {
        res.writeHead(200, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ object: "list", data: [{ id: "gpt-lab" }, { id: "gpt-lab-mini" }, { id: "text-embedding-lab" }] }));
        return;
      }
      let body: any = {};
      try {
        body = JSON.parse(raw);
      } catch {}
      heard.push({ url: req.url ?? "", body });
      if (req.url?.endsWith("/embeddings")) {
        const input: string[] = Array.isArray(body.input) ? body.input : [body.input];
        res.writeHead(200, { "Content-Type": "application/json" });
        res.end(JSON.stringify({
          object: "list",
          data: input.map((t, index) => ({ object: "embedding", index, embedding: [t.length, (t.match(/[aeiou]/g) ?? []).length, 0.5] })),
          usage: { prompt_tokens: input.length * 3, total_tokens: input.length * 3 },
        }));
        return;
      }
      if (req.url?.endsWith("/completions") && !req.url.endsWith("/chat/completions")) {
        res.writeHead(200, { "Content-Type": "text/event-stream" });
        res.write(mk({ choices: [{ index: 0, text: "<grown>" }] }));
        res.write(mk({ choices: [{ index: 0, text: body.suffix ? `>${body.suffix.length}` : "" , finish_reason: "stop" }] }));
        res.write(mk({ choices: [], usage: { prompt_tokens: 5, completion_tokens: 2, total_tokens: 7 } }));
        res.end("data: [DONE]\n\n");
        return;
      }
      const msgs: any[] = body.messages ?? [];
      const last = msgs[msgs.length - 1] ?? {};
      const said = lastUser(msgs);
      const asked = new Set<string>();
      for (const m of msgs) {
        if (m.role === "assistant") for (const c of m.tool_calls ?? []) asked.add(c.id);
        if (m.role === "tool") asked.delete(m.tool_call_id);
      }
      if (asked.size > 0) {
        res.writeHead(400);
        res.end(JSON.stringify({ error: { message: `tool calls nobody answered: ${[...asked].join(",")}` } }));
        return;
      }
      if (said.includes("FAIL")) {
        res.writeHead(500);
        res.end(JSON.stringify({ error: { message: "upstream exploded" } }));
        return;
      }
      res.writeHead(200, { "Content-Type": "text/event-stream" });
      let gone = false;
      res.on("close", () => {
        if (!res.writableFinished && !gone) {
          gone = true;
          aborted.push(said);
        }
      });
      const call = (index: number, id: string, name: string, args: string) =>
        res.write(mk({ choices: [{ index: 0, delta: { tool_calls: [{ index, id, type: "function", function: { name, arguments: args } }] } }] }));
      const tooling = last.role !== "tool";
      if (tooling && said.includes("PAR")) {
        call(0, "call_a", "readFile", '{"path":"a"}');
        call(1, "call_b", "readFile", '{"path":"b"}');
        res.write(mk({ choices: [{ index: 0, delta: {}, finish_reason: "tool_calls" }] }));
      } else if (tooling && said.includes("ONE")) {
        call(0, "call_one", "readFile", '{"path":"x"}');
        res.write(mk({ choices: [{ index: 0, delta: {}, finish_reason: "tool_calls" }] }));
      } else if (tooling && said.includes("NOTFOUND")) {
        call(0, "call_none", "lookup", '{"q":"nothing"}');
        res.write(mk({ choices: [{ index: 0, delta: {}, finish_reason: "tool_calls" }] }));
      } else if (tooling && said.includes("LOOKUP")) {
        call(0, "call_look", "lookup", '{"q":"bees"}');
        res.write(mk({ choices: [{ index: 0, delta: {}, finish_reason: "tool_calls" }] }));
      } else if (tooling && /weather/i.test(said) && body.tools?.length > 0) {
        const tool = body.tools.find((t: any) => /weather/i.test(t.function?.name ?? "")) ?? body.tools[0];
        call(0, "call_weather", tool.function.name, '{"location":"San Francisco, CA"}');
        res.write(mk({ choices: [{ index: 0, delta: {}, finish_reason: "tool_calls" }] }));
      } else if (said.includes("SLOW")) {
        for (let i = 0; i < 20 && !gone; i++) {
          res.write(mk({ choices: [{ index: 0, delta: { content: `tick${i} ` } }] }));
          await sleep(250);
        }
        if (!gone) res.write(mk({ choices: [{ index: 0, delta: {}, finish_reason: "stop" }] }));
      } else if (said.includes("LEN")) {
        res.write(mk({ choices: [{ index: 0, delta: { content: "cut sho" } }] }));
        res.write(mk({ choices: [{ index: 0, delta: {}, finish_reason: "length" }] }));
      } else {
        const text = last.role === "tool"
          ? `after tools: ${msgs.filter((m) => m.role === "tool").map((m) => m.content).join(" | ")}`
          : `echo: ${said.slice(0, 80)}`;
        res.write(mk({ choices: [{ index: 0, delta: { role: "assistant", reasoning_content: "thinking" } }] }));
        res.write(mk({ choices: [{ index: 0, delta: { content: text } }] }));
        res.write(mk({ choices: [{ index: 0, delta: {}, finish_reason: "stop" }] }));
        res.write(mk({ choices: [], usage: { prompt_tokens: 11, completion_tokens: 7, total_tokens: 18, prompt_tokens_details: { cached_tokens: 4 } } }));
      }
      if (!gone) res.end("data: [DONE]\n\n");
    });
  });
  await new Promise<void>((r) => server.listen(at, "127.0.0.1", r));
  const port = (server.address() as { port: number }).port;
  return {
    url: `http://127.0.0.1:${port}/v1`,
    heard,
    aborted,
    close: () => new Promise<void>((r) => {
      server.closeAllConnections();
      server.close(() => r());
    }),
  };
}

export async function upstream(): Promise<Upstream> {
  const child = spawn("node", [import.meta.filename, "serve"], { stdio: ["pipe", "pipe", "inherit"] });
  const url = await new Promise<string>((resolve, reject) => {
    child.stdout!.once("data", (b) => resolve(String(b).trim()));
    child.once("exit", (code) => reject(new Error(`the scripted upstream exited ${code}`)));
  });
  const root = url.replace(/\/v1$/, "");
  return {
    url,
    log: async () => (await (await fetch(`${root}/_heard`)).json()) as { heard: Heard[]; aborted: string[] },
    close: async () => {
      child.kill("SIGTERM");
      await new Promise((r) => (child.exitCode !== null ? r(null) : child.once("exit", r)));
    },
  };
}

if (process.argv[2] === "serve") {
  const up = await serve();
  console.log(up.url);
  process.stdin.once("end", () => process.exit(0));
  process.stdin.resume();
}

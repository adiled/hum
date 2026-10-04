
import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { createServer as createNetServer, type Server, type Socket } from "node:net";
import { createServer as createHttpServer } from "node:http";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawn, type Subprocess } from "bun";

type Tone = Record<string, unknown>;

async function startFakeOpenAI(toolCallsFirstTurn: boolean): Promise<{ port: number; shutdown: () => void }> {
  const server = createHttpServer((req, res) => {
    let body = "";
    req.on("data", (c) => { body += c.toString(); });
    req.on("end", () => {
      if (req.method === "POST" && req.url?.endsWith("/chat/completions")) {
        res.writeHead(200, { "Content-Type": "text/event-stream" });
        const mk = (o: object) => `data: ${JSON.stringify(o)}\n\n`;
        if (toolCallsFirstTurn && !body.includes('"role":"tool"')) {
          res.write(mk({ choices: [{ delta: { role: "assistant", tool_calls: [{ index: 0, id: "call_abc", type: "function", function: { name: "read", arguments: "{\"path\":\"" } }] } }] }));
          res.write(mk({ choices: [{ delta: { tool_calls: [{ index: 0, function: { arguments: "a.txt\"}" } }] } }] }));
        } else {
          res.write(mk({ choices: [{ delta: { content: "saw the" } }] }));
          res.write(mk({ choices: [{ delta: { content: " file" } }] }));
          res.write(mk({ usage: { prompt_tokens: 3, completion_tokens: 2 } }));
        }
        res.write("data: [DONE]\n\n");
        res.end();
      } else {
        res.writeHead(404);
        res.end();
      }
    });
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  const port = (server.address() as { port: number }).port;
  return { port, shutdown: () => server.close() };
}

async function startFakeHumd(): Promise<{
  sockPath: string;
  capturedReplies: Tone[];
  helloReceived: Promise<Tone>;
  send: (t: Tone) => void;
  shutdown: () => Promise<void>;
}> {
  const dir = mkdtempSync(join(tmpdir(), "hum-worker-e2e-"));
  const sockPath = join(dir, "thrum.sock");
  const capturedReplies: Tone[] = [];
  let helloResolve!: (t: Tone) => void;
  const helloReceived = new Promise<Tone>((r) => { helloResolve = r; });
  let active: Socket | null = null;

  const server: Server = createNetServer((sock) => {
    active = sock;
    let buf = "";
    sock.on("data", (chunk) => {
      buf += chunk.toString();
      let nl: number;
      while ((nl = buf.indexOf("\n")) >= 0) {
        const line = buf.slice(0, nl);
        buf = buf.slice(nl + 1);
        if (!line) continue;
        try {
          const tone = JSON.parse(line) as Tone;
          if (tone.chi === "hello") { helloResolve(tone); continue; }
          capturedReplies.push(tone);
        } catch {}
      }
    });
  });
  await new Promise<void>((res, rej) => {
    server.once("error", rej);
    server.listen(sockPath, () => res());
  });

  return {
    sockPath,
    capturedReplies,
    helloReceived,
    send: (t) => { if (active) active.write(JSON.stringify(t) + "\n"); },
    shutdown: async () => {
      if (active) active.destroy();
      await new Promise<void>((r) => server.close(() => r()));
      rmSync(dir, { recursive: true, force: true });
    },
  };
}

async function waitForHello(hello: Promise<Tone>, timeoutMs: number): Promise<Tone> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const h = await Promise.race([
        hello,
        new Promise<never>((_, rej) => setTimeout(() => rej(new Error("timeout")), 500)),
      ]);
      return h;
    } catch { await new Promise((r) => setTimeout(r, 50)); }
  }
  throw new Error("bee never helloed");
}

async function waitFor(pred: () => boolean, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline && !pred()) {
    await new Promise((r) => setTimeout(r, 25));
  }
}

let openAI: { port: number; shutdown: () => void };
let humd: Awaited<ReturnType<typeof startFakeHumd>>;
let server: Subprocess<"ignore", "pipe", "pipe">;

async function boot(toolCallsFirstTurn: boolean) {
  openAI = await startFakeOpenAI(toolCallsFirstTurn);
  humd = await startFakeHumd();
  server = spawn({
    cmd: ["bun", "src/index.ts"],
    cwd: import.meta.dir + "/..",
    env: {
      ...process.env,
      HUM_THRUM_SOCK: humd.sockPath,
      OPENAI_SERVER_PORT: "14629",
      OPENAI_SERVER_API_KEY: "",
      OPENAI_API_BASE: `http://127.0.0.1:${openAI.port}/v1`,
      OPENAI_API_KEY: "sk-test",
    },
    stdout: "pipe",
    stderr: "pipe",
  });
  await waitForHello(humd.helloReceived, 5000);
}

async function teardown() {
  server.kill();
  await server.exited;
  await humd.shutdown();
  openAI.shutdown();
}

describe("openai-server worker half — chi:prompt in, chunk/finish out", () => {
  beforeAll(async () => { await boot(false); });
  afterAll(async () => { await teardown(); });

  test("streams text_delta chunks and a finish with usage", async () => {
    const sid = "worker-test-sid";
    humd.send({ chi: "prompt", sid, hive: "openai-server", modelId: "gpt-4o", content: "hello worker" });

    await waitFor(() => humd.capturedReplies.some(r => r.chi === "finish"), 5000);

    const textDeltas = humd.capturedReplies.filter(r => r.chi === "chunk" && r.chunkType === "text_delta");
    expect(textDeltas.length).toBeGreaterThan(0);
    for (const c of textDeltas) expect(c.sid).toBe(sid);

    const text = textDeltas.map(c => c.delta).join("");
    expect(text).toBe("saw the file");

    const finish = humd.capturedReplies.find(r => r.chi === "finish");
    expect(finish!.sid).toBe(sid);
    expect(finish!.finishReason).toBe("stop");
    expect(finish!.usage).toMatchObject({ input_tokens: 3, output_tokens: 2 });
  });
});

describe("openai-server worker half — tool loop", () => {
  beforeAll(async () => { await boot(true); });
  afterAll(async () => { await teardown(); });

  test("tool_calls → chi:tool-call, tool-result → resumes and finishes", async () => {
    const sid = "worker-tool-sid";
    humd.send({ chi: "prompt", sid, hive: "openai-server", modelId: "gpt-4o", content: "read the file", tools: [{ name: "read", description: "Read a file", inputSchema: { type: "object" } }] });

    await waitFor(() => humd.capturedReplies.some(r => r.chi === "tool-call"), 5000);
    expect(humd.capturedReplies.some(r => r.chi === "finish")).toBe(false);

    const toolCall = humd.capturedReplies.find(r => r.chi === "tool-call")!;
    expect(toolCall.sid).toBe(sid);
    expect(toolCall.toolName).toBe("read");
    expect(toolCall.callId).toBeDefined();
    expect(toolCall.args).toContain("a.txt");

    humd.send({ chi: "tool-result", sid, callId: toolCall.callId, toolName: "read", output: "file contents" });

    await waitFor(() => humd.capturedReplies.some(r => r.chi === "finish"), 5000);

    const textDeltas = humd.capturedReplies.filter(r => r.chi === "chunk" && r.chunkType === "text_delta");
    const text = textDeltas.map(c => c.delta).join("");
    expect(text).toBe("saw the file");

    const finish = humd.capturedReplies.find(r => r.chi === "finish")!;
    expect(finish.finishReason).toBe("stop");
  });
});

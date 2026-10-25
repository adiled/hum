
import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { createServer as createNetServer, type Server, type Socket } from "node:net";
import { createServer as createHttpServer } from "node:http";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawn, type Subprocess } from "bun";

type Tone = Record<string, unknown>;

async function startFakeOpenAI(toolCallsFirst: boolean): Promise<{ port: number; shutdown: () => void }> {
  const server = createHttpServer((req, res) => {
    let body = "";
    req.on("data", (c) => { body += c.toString(); });
    req.on("end", () => {
      if (req.method === "GET" && req.url?.endsWith("/models")) {
        res.writeHead(200, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ object: "list", data: [{ id: "gpt-4o", object: "model", created: 0, owned_by: "fake" }] }));
      } else if (req.method === "POST" && req.url?.endsWith("/chat/completions")) {
        res.writeHead(200, { "Content-Type": "text/event-stream" });
        const mk = (o: object) => `data: ${JSON.stringify(o)}\n\n`;
        if (toolCallsFirst && !body.includes('"role":"tool"')) {
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
  const dir = mkdtempSync(join(tmpdir(), "hum-nurse-e2e-"));
  const sockPath = join(dir, "thrum.sock");
  const capturedReplies: Tone[] = [];
  let helloResolve!: (t: Tone) => void;
  const helloReceived = new Promise<Tone>((r) => { helloResolve = r; });
  let active: Socket | null = null;

  const server: Server = createNetServer((sock) => {
    active = sock;
    let buf = "";
    sock.write(JSON.stringify({ chi: "breath", sigils: [] }) + "\n");
    sock.on("data", (chunk) => {
      buf += chunk.toString();
      let nl: number;
      while ((nl = buf.indexOf("\n")) >= 0) {
        const line = buf.slice(0, nl);
        buf = buf.slice(nl + 1);
        if (!line) continue;
        try {
          const tone = JSON.parse(line) as Tone;
          if (tone.chi === "hello") {
            sock.write(JSON.stringify({ chi: "echo", rid: tone.rid, ok: true }) + "\n");
            sock.write(JSON.stringify({ chi: "breath", sigils: {} }) + "\n");
            helloResolve(tone);
            continue;
          }
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

async function boot(toolCallsFirst: boolean) {
  openAI = await startFakeOpenAI(toolCallsFirst);
  humd = await startFakeHumd();
  const xdg = mkdtempSync(join(tmpdir(), "hum-nurse-xdg-"));
  mkdirSync(join(xdg, "config", "hum", "hives.d"), { recursive: true });
  writeFileSync(join(xdg, "config", "hum", "hives.d", "openai-server.orch"), "");
  const humBin = join(import.meta.dir, "..", "..", "..", "target", "debug");
  server = spawn({
    cmd: ["bun", "src/index.ts"],
    cwd: import.meta.dir + "/..",
    env: {
      ...process.env,
      HUM_THRUM_SOCK: humd.sockPath,
      XDG_CONFIG_HOME: join(xdg, "config"),
      XDG_STATE_HOME: join(xdg, "state"),
      PATH: `${humBin}:${process.env.PATH ?? ""}`,
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

describe("openai-server nurse half — chi:prompt in, a bloom out", () => {
  beforeAll(async () => { await boot(false); });
  afterAll(async () => { await teardown(); });

  test("hello advertises the nurse's discovered models", async () => {
    const hello = await waitForHello(humd.helloReceived, 5000);
    expect(hello.models).toEqual(["gpt-4o"]);
  });

  test("unfurls a text petal and wilts with nectar", async () => {
    const sid = "nurse-test-sid";
    humd.send({ chi: "prompt", sid, hive: "openai-server", modelId: "gpt-4o", content: [{ kind: "text", text: "hello nurse" }] });

    await waitFor(() => humd.capturedReplies.some(r => r.chi === "wilt"), 5000);

    const unfurled = humd.capturedReplies.filter(r => r.chi === "unfurl" && r.kind === "text" && typeof r.text === "string");
    expect(unfurled.length).toBeGreaterThan(0);
    for (const u of unfurled) expect(u.sid).toBe(sid);

    const text = unfurled.map(u => u.text).join("");
    expect(text).toBe("saw the file");
    expect(humd.capturedReplies.some(r => r.chi === "unfurl" && r.closed === true)).toBe(true);

    const wilt = humd.capturedReplies.find(r => r.chi === "wilt");
    expect(wilt!.sid).toBe(sid);
    expect(wilt!.reason).toBe("stop");
    expect(wilt!.nectar).toMatchObject({ input_tokens: 3, output_tokens: 2 });
  });
});

describe("openai-server nurse half — tool loop", () => {
  beforeAll(async () => { await boot(true); });
  afterAll(async () => { await teardown(); });

  test("tool_calls → chi:tendril-reach, tendril-result → it grows on and wilts", async () => {
    const sid = "nurse-tool-sid";
    humd.send({ chi: "prompt", sid, hive: "openai-server", modelId: "gpt-4o", content: [{ kind: "text", text: "read the file" }], tools: [{ name: "read", description: "Read a file", inputSchema: { type: "object" } }] });

    await waitFor(() => humd.capturedReplies.some(r => r.chi === "tendril-reach"), 5000);
    expect(humd.capturedReplies.some(r => r.chi === "wilt")).toBe(false);

    const sap = humd.capturedReplies.filter(r => r.chi === "unfurl" && r.kind === "tendril").map(r => r.sap ?? "").join("");
    expect(sap).toBe("{\"path\":\"a.txt\"}");

    const reach = humd.capturedReplies.find(r => r.chi === "tendril-reach")!;
    expect(reach.sid).toBe(sid);
    expect(reach.name).toBe("read");
    expect(reach.callId).toBeDefined();
    expect(reach.args).toEqual({ path: "a.txt" });

    humd.send({ chi: "tendril-result", sid, callId: reach.callId, output: "file contents" });

    await waitFor(() => humd.capturedReplies.some(r => r.chi === "wilt"), 5000);

    const text = humd.capturedReplies
      .filter(r => r.chi === "unfurl" && r.kind === "text" && typeof r.text === "string")
      .map(r => r.text)
      .join("");
    expect(text).toBe("saw the file");

    const wilt = humd.capturedReplies.find(r => r.chi === "wilt")!;
    expect(wilt.reason).toBe("stop");
  });
});

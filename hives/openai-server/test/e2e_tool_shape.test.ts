// e2e — openai-server ↔ thrum wire contract for tools.
//
// Spawns the real openai-server subprocess against a fake humd
// (a unix-socket sink the test owns). Drives /v1/chat/completions
// with OpenAI-style tools and asserts the chi:"prompt" tone humd
// receives carries `tools[*].inputSchema` as a non-null object.
//
// Pins the bug we hit: OC ships `parameters`, server must
// translate to `inputSchema` because claude's mcp client rejects
// the whole tools/list when any entry has `inputSchema: null`.
// Anywhere along this wire that drops the schema, this test fails.

import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { createServer, type Server, type Socket } from "node:net";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawn, type Subprocess } from "bun";

type Tone = Record<string, unknown>;

interface FakeHumd {
  sockPath: string;
  capturedPrompts: Tone[];
  helloReceived: Promise<Tone>;
  shutdown: () => Promise<void>;
}

// Bind a UDS server that accepts ONE openai-server connection,
// captures the hello + every chi:"prompt" tone, and on each
// prompt replies with a minimal chunk + finish sequence so the
// HTTP response can complete.
async function startFakeHumd(): Promise<FakeHumd> {
  const dir = mkdtempSync(join(tmpdir(), "hum-e2e-"));
  const sockPath = join(dir, "thrum.sock");
  const capturedPrompts: Tone[] = [];
  let helloResolve!: (t: Tone) => void;
  const helloReceived = new Promise<Tone>((r) => { helloResolve = r; });
  let active: Socket | null = null;

  const server: Server = createServer((sock) => {
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
          if (tone.chi === "prompt") {
            capturedPrompts.push(tone);
            const sid = String(tone.sid ?? "");
            // Reply with the minimum frame set translator.ingest
            // needs to produce a completed response: an init
            // session, one text delta, then a finish.
            const frames: Tone[] = [
              { chi: "session-ready", sid, nestId: "fake-nest", model: tone.modelId, tools: [] },
              { chi: "chunk", sid, chunkType: "text_start", id: 0 },
              { chi: "chunk", sid, chunkType: "text_delta", delta: "ok" },
              { chi: "chunk", sid, chunkType: "content_block_stop", blockIdx: 0 },
              { chi: "finish", sid, finishReason: "stop", usage: { input_tokens: 1, output_tokens: 1 } },
            ];
            for (const f of frames) sock.write(JSON.stringify(f) + "\n");
          }
        } catch { /* ignore parse */ }
      }
    });
  });
  await new Promise<void>((res, rej) => {
    server.once("error", rej);
    server.listen(sockPath, () => res());
  });

  return {
    sockPath,
    capturedPrompts,
    helloReceived,
    shutdown: async () => {
      if (active) active.destroy();
      await new Promise<void>((r) => server.close(() => r()));
      rmSync(dir, { recursive: true, force: true });
    },
  };
}

async function waitForHttp(url: string, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const r = await fetch(url, { signal: AbortSignal.timeout(500) });
      if (r.status < 500) return;
    } catch { /* not up yet */ }
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error(`server at ${url} never came up`);
}

let humd: FakeHumd;
let server: Subprocess<"ignore", "pipe", "pipe">;
const PORT = 14629; // unlikely to clash with the deployed 14620
const BASE = `http://127.0.0.1:${PORT}`;

beforeAll(async () => {
  humd = await startFakeHumd();
  server = spawn({
    cmd: ["bun", "src/index.ts"],
    cwd: import.meta.dir + "/..",
    env: {
      ...process.env,
      HUM_THRUM_SOCK: humd.sockPath,
      OPENAI_SERVER_PORT: String(PORT),
      OPENAI_SERVER_API_KEY: "", // disable auth
    },
    stdout: "pipe",
    stderr: "pipe",
  });
  await humd.helloReceived;
  await waitForHttp(`${BASE}/v1/models`, 5000);
});

afterAll(async () => {
  server.kill();
  await server.exited;
  await humd.shutdown();
});

describe("openai-server → humd tools wire shape", () => {
  test("tools[*].parameters from OpenAI → tools[*].inputSchema on thrum", async () => {
    const before = humd.capturedPrompts.length;
    const res = await fetch(`${BASE}/v1/chat/completions`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        model: "claude-opus-4-7",
        stream: false,
        user: "e2e-tool-shape",
        messages: [{ role: "user", content: "hi" }],
        tools: [
          {
            type: "function",
            function: {
              name: "read",
              description: "Read a file",
              parameters: {
                type: "object",
                properties: { path: { type: "string" } },
                required: ["path"],
              },
            },
          },
          {
            type: "function",
            function: { name: "ping" }, // no parameters — must still emit a schema
          },
        ],
      }),
    });
    expect(res.status).toBe(200);
    // Wait briefly for thrum-side prompt capture.
    const deadline = Date.now() + 3000;
    while (humd.capturedPrompts.length === before && Date.now() < deadline) {
      await new Promise((r) => setTimeout(r, 20));
    }
    expect(humd.capturedPrompts.length).toBeGreaterThan(before);

    const prompt = humd.capturedPrompts[humd.capturedPrompts.length - 1];
    const tools = prompt.tools as Array<Record<string, unknown>> | undefined;
    expect(Array.isArray(tools)).toBe(true);
    expect(tools).toHaveLength(2);

    for (const t of tools!) {
      expect(t).toHaveProperty("inputSchema");
      // Anything-but-null. claude's mcp client zod-validates.
      expect(t.inputSchema).not.toBeNull();
      expect(typeof t.inputSchema).toBe("object");
      // Bug guard: the legacy field name MUST be gone.
      expect((t as Record<string, unknown>).parameters).toBeUndefined();
    }

    const read = tools!.find((t) => t.name === "read")!;
    expect(read.inputSchema).toEqual({
      type: "object",
      properties: { path: { type: "string" } },
      required: ["path"],
    });
    const ping = tools!.find((t) => t.name === "ping")!;
    expect(ping.inputSchema).toEqual({});
  });
});

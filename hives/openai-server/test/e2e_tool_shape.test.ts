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
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawn, type Subprocess } from "bun";

type Tone = Record<string, unknown>;

interface FakeHumd {
  sockPath: string;
  capturedPrompts: Tone[];
  helloReceived: Promise<Tone>;
  hellos: Tone[];
  promptBees: string[];
  shutdown: () => Promise<void>;
}

async function startFakeHumd(): Promise<FakeHumd> {
  const dir = mkdtempSync(join(tmpdir(), "hum-e2e-"));
  const sockPath = join(dir, "thrum.sock");
  const capturedPrompts: Tone[] = [];
  const hellos: Tone[] = [];
  const promptBees: string[] = [];
  let helloResolve!: (t: Tone) => void;
  const helloReceived = new Promise<Tone>((r) => { helloResolve = r; });
  let active: Socket | null = null;

  const server: Server = createServer((sock) => {
    active = sock;
    let buf = "";
    let bee = "";
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
            hellos.push(tone);
            bee = String(tone.hid ?? "");
            sock.write(JSON.stringify({ chi: "echo", rid: tone.rid, ok: true }) + "\n");
            sock.write(JSON.stringify({ chi: "breath", sigils: {} }) + "\n");
            helloResolve(tone);
            continue;
          }
          if (tone.chi === "prompt") {
            capturedPrompts.push(tone);
            promptBees.push(bee);
            const sid = String(tone.sid ?? "");
            const frames: Tone[] = [
              { chi: "pulse", sid, kind: "cell-ready", cell: "fake-cell", model: tone.modelId, tools: [] },
              { chi: "unfurl", sid, petal: 0, kind: "text" },
              { chi: "unfurl", sid, petal: 0, kind: "text", text: "ok" },
              { chi: "unfurl", sid, petal: 0, kind: "text", closed: true },
              { chi: "wilt", sid, reason: "stop", nectar: { input_tokens: 1, output_tokens: 1 } },
            ];
            for (const f of frames) sock.write(JSON.stringify(f) + "\n");
          }
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
    capturedPrompts,
    helloReceived,
    hellos,
    promptBees,
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
    } catch {}
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error(`server at ${url} never came up`);
}

let humd: FakeHumd;
let server: Subprocess<"ignore", "pipe", "pipe">;
const PORT = 14629;
const BASE = `http://127.0.0.1:${PORT}`;

beforeAll(async () => {
  humd = await startFakeHumd();
  const xdg = mkdtempSync(join(tmpdir(), "hum-e2e-xdg-"));
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
      OPENAI_SERVER_PORT: String(PORT),
      OPENAI_SERVER_API_KEY: "",
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
            function: { name: "ping" },
          },
        ],
      }),
    });
    expect(res.status).toBe(200);
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
      expect(t.inputSchema).not.toBeNull();
      expect(typeof t.inputSchema).toBe("object");
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

describe("every requester is a bee", () => {
  async function ask(user: string): Promise<void> {
    const r = await fetch(`${BASE}/v1/chat/completions`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ model: "claude-opus-4-7", stream: false, user, messages: [{ role: "user", content: `hi from ${user}` }] }),
    });
    expect(r.status).toBe(200);
    await r.text();
  }

  test("two requesters are two forager bees, and one requester stays one bee", async () => {
    const before = humd.promptBees.length;
    await ask("alice");
    await ask("bob");
    await ask("alice");
    const [a1, b, a2] = humd.promptBees.slice(before);
    const main = String((await humd.helloReceived).hid);
    expect(a1).toMatch(/^bee_[0-9a-f]{64}$/);
    expect(b).toMatch(/^bee_[0-9a-f]{64}$/);
    expect(a1).not.toBe(b);
    expect(a2).toBe(a1);
    expect(a1).not.toBe(main);
    const foragers = humd.hellos.filter((h) => h.role === "forager");
    expect(foragers.length).toBeGreaterThanOrEqual(2);
    for (const h of humd.hellos) expect(typeof h.signature).toBe("string");
  });
});

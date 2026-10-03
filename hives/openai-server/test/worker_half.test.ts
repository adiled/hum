// worker-half — the inverse direction of openai-server.
//
// The forager half translates OpenAI HTTP wire → chi:"prompt" out to
// humd. This test drives the worker half the other way: a fake humd
// routes a chi:"prompt" tone BACK to this bee (humd forwards prompts
// whose modelId matches an advertised model), the worker calls the
// upstream OpenAI API, and streams chi:"chunk"/"finish" tones out.
//
// We stand up a fake upstream OpenAI-compatible server (via
// OPENAI_API_BASE) that returns a canned SSE chat-completions stream,
// and a fake humd that sends one prompt and captures the reply tones.
//
// Pins the wire: chunks must carry sid + chunkType + delta, and a
// finish must carry finishReason + usage — the canonical surface the
// Rust WireListener emits, so downstream foragers/consumers see the
// same shapes regardless of which bee produced them.

import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { createServer as createNetServer, type Server, type Socket } from "node:net";
import { createServer as createHttpServer } from "node:http";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawn, type Subprocess } from "bun";

type Tone = Record<string, unknown>;

// Fake upstream OpenAI-compatible API: answers POST /chat/completions
// with a streaming SSE chat-completions body.
async function startFakeOpenAI(): Promise<{ port: number; shutdown: () => void }> {
  const server = createHttpServer((req, res) => {
    let body = "";
    req.on("data", (c) => { body += c.toString(); });
    req.on("end", () => {
      if (req.method === "POST" && (req.url?.endsWith("/chat/completions"))) {
        res.writeHead(200, { "Content-Type": "text/event-stream" });
        const mk = (o: object) => `data: ${JSON.stringify(o)}\n\n`;
        res.write(mk({ choices: [{ delta: { content: "hello" } }] }));
        res.write(mk({ choices: [{ delta: { content: " from the" } }] }));
        res.write(mk({ choices: [{ delta: { content: " upstream" } }] }));
        res.write(mk({ usage: { prompt_tokens: 3, completion_tokens: 3 } }));
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

// Fake humd: accepts one openai-server connection, captures hello,
// then sends a chi:"prompt" tone and records every reply tone.
async function startFakeHumd(): Promise<{
  sockPath: string;
  capturedReplies: Tone[];
  helloReceived: Promise<Tone>;
  sendPrompt: (t: Tone) => void;
  shutdown: () => Promise<void>;
}> {
  const dir = mkdtempSync(join(tmpdir(), "hum-worker-e2e-"));
  const sockPath = join(dir, "thrum.sock");
  const capturedReplies: Tone[] = [];
  let helloResolve!: (t: Tone) => void;
  const helloReceived = new Promise<Tone>((r) => { helloResolve = r; });
  let active: Socket | null = null;
  let helloDone = false;

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
          if (tone.chi === "hello") {
            helloResolve(tone);
            helloDone = true;
            continue;
          }
          // Any non-hello tone the bee emits is a reply to our prompt.
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
    sendPrompt: (t) => { if (active) active.write(JSON.stringify(t) + "\n"); },
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

let openAI: { port: number; shutdown: () => void };
let humd: ReturnType<typeof startFakeHumd> extends Promise<infer T> ? T : never;
let server: Subprocess<"ignore", "pipe", "pipe">;

beforeAll(async () => {
  openAI = await startFakeOpenAI();
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
});

afterAll(async () => {
  server.kill();
  await server.exited;
  await humd.shutdown();
  openAI.shutdown();
});

describe("openai-server worker half — chi:prompt in, chunk/finish out", () => {
  test("streams text_delta chunks and a finish with usage", async () => {
    const sid = "worker-test-sid";
    humd.sendPrompt({
      chi: "prompt",
      sid,
      hive: "openai-server",
      modelId: "gpt-4o",
      content: "hello worker",
    });

    // Wait for the reply tones to arrive.
    const deadline = Date.now() + 5000;
    while (Date.now() < deadline && !humd.capturedReplies.some(r => r.chi === "finish")) {
      await new Promise((r) => setTimeout(r, 25));
    }

    const textDeltas = humd.capturedReplies.filter(r => r.chi === "chunk" && r.chunkType === "text_delta");
    expect(textDeltas.length).toBeGreaterThan(0);
    // All chunks carry the original sid.
    for (const c of textDeltas) expect(c.sid).toBe(sid);

    // Concatenate the deltas into the upstream reply.
    const text = textDeltas.map(c => c.delta).join("");
    expect(text).toBe("hello from the upstream");

    const finish = humd.capturedReplies.find(r => r.chi === "finish");
    expect(finish).toBeDefined();
    expect(finish!.sid).toBe(sid);
    expect(finish!.finishReason).toBe("stop");
    expect(finish!.usage).toMatchObject({ input_tokens: 3, output_tokens: 3 });
  });
});

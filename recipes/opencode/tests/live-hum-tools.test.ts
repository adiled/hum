// recipes/opencode/tests/live-hum-tools.test.ts
//
// Live integration: hits the running hum-openai-server endpoint
// directly, the same way OC does. Asserts the full tool-use
// round-trip works against a real deployment:
//
//   POST /v1/chat/completions (with tools[])
//     → openai-server: rename parameters→inputSchema, forward chi:prompt
//     → humd: merge forager catalogue, route to worker bee
//     → worker MCP bridge: serve tools/list (no null schemas — was the bug)
//     → worker's claude: pick a tool, call via MCP bridge, get result
//     → worker's claude: continue, emit text, finish=stop
//     → openai-server: stream SSE with tool_calls + content + [DONE]
//
// Pre-req: `dev/deploy` has run on this box; humd + worker + humfs
// forager + openai-server are all live as systemd units under the
// HUM_DEV_USER. The test skips if openai-server isn't reachable —
// no auto-start. This is meant to run against a stable deploy.

import { describe, expect, test, beforeAll } from "vitest";

const BASE = process.env.HUM_OPENAI_BASE ?? "http://127.0.0.1:14620/v1";

interface SseChunk {
  id: string;
  choices?: Array<{
    delta?: {
      content?: string;
      tool_calls?: Array<{
        index: number;
        id?: string;
        type?: string;
        function?: { name?: string; arguments?: string };
      }>;
    };
    finish_reason?: string | null;
  }>;
  usage?: Record<string, number>;
}

interface Collected {
  content: string;
  toolCalls: Array<{ id: string; name: string; arguments: string }>;
  finishReason: string | null;
  saw: { reasoningChunks: number; contentChunks: number; toolChunks: number };
  rawDoneAt: number;
}

async function streamChat(body: Record<string, unknown>, timeoutMs: number): Promise<Collected> {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), timeoutMs);
  try {
    const r = await fetch(`${BASE}/chat/completions`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ stream: true, ...body }),
      signal: ctrl.signal,
    });
    if (!r.ok) throw new Error(`/chat/completions returned ${r.status}: ${await r.text()}`);
    if (!r.body) throw new Error("no response body");
    const reader = r.body.getReader();
    const dec = new TextDecoder();
    let buf = "";
    const out: Collected = {
      content: "",
      toolCalls: [],
      finishReason: null,
      saw: { reasoningChunks: 0, contentChunks: 0, toolChunks: 0 },
      rawDoneAt: 0,
    };
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      buf += dec.decode(value, { stream: true });
      let nl: number;
      while ((nl = buf.indexOf("\n\n")) >= 0) {
        const block = buf.slice(0, nl);
        buf = buf.slice(nl + 2);
        if (!block.startsWith("data: ")) continue;
        const payload = block.slice(6);
        if (payload === "[DONE]") {
          out.rawDoneAt = Date.now();
          continue;
        }
        let chunk: SseChunk;
        try { chunk = JSON.parse(payload) as SseChunk; } catch { continue; }
        const choice = chunk.choices?.[0];
        if (!choice) continue;
        if (choice.finish_reason) out.finishReason = choice.finish_reason;
        const d = choice.delta ?? {};
        if (typeof (d as any).reasoning_content === "string" && (d as any).reasoning_content.length > 0) {
          out.saw.reasoningChunks++;
        }
        if (typeof d.content === "string" && d.content.length > 0) {
          out.content += d.content;
          out.saw.contentChunks++;
        }
        if (Array.isArray(d.tool_calls)) {
          for (const tc of d.tool_calls) {
            const slot = out.toolCalls[tc.index] ?? { id: "", name: "", arguments: "" };
            if (tc.id) slot.id = tc.id;
            if (tc.function?.name) slot.name = tc.function.name;
            if (tc.function?.arguments) slot.arguments += tc.function.arguments;
            out.toolCalls[tc.index] = slot;
            out.saw.toolChunks++;
          }
        }
      }
    }
    return out;
  } finally {
    clearTimeout(timer);
  }
}

let alive = false;
beforeAll(async () => {
  try {
    const r = await fetch(`${BASE}/models`, { signal: AbortSignal.timeout(2000) });
    alive = r.ok;
  } catch { alive = false; }
});

function skipIfDead() {
  if (!alive) (test as any).skip(`hum-openai-server at ${BASE} not reachable — run dev/deploy`);
}

describe("live hum-openai-server: tool-use round-trip", () => {
  test("models endpoint advertises at least one claude model", async () => {
    skipIfDead();
    const r = await fetch(`${BASE}/models`);
    const j = await r.json() as { data: Array<{ id: string }> };
    expect(Array.isArray(j.data)).toBe(true);
    expect(j.data.length).toBeGreaterThan(0);
    expect(j.data.some((m) => m.id.includes("claude"))).toBe(true);
  });

  // The bug: tools came in as `parameters`, landed on the wire as
  // `inputSchema: null`, claude's mcp client rejected the entire
  // tools/list, model hallucinated Claude-Code-trained names like
  // `agent`. If that path is still broken the model can't invoke
  // any tool — including ones we pass here.
  test("tool-call + result flow into final text response", async () => {
    skipIfDead();
    const result = await streamChat({
      model: "claude-haiku-4-5",
      messages: [{ role: "user", content: "Use the read tool to read /etc/hostname. Then state the hostname plainly." }],
      tools: [{
        type: "function",
        function: {
          name: "read",
          description: "Read a file from disk",
          parameters: {
            type: "object",
            properties: { path: { type: "string", description: "Absolute path" } },
            required: ["path"],
          },
        },
      }],
    }, 90_000);

    // Stream closed cleanly.
    expect(result.rawDoneAt, "SSE ended with [DONE]").toBeGreaterThan(0);
    // finish_reason landed.
    expect(result.finishReason, "stream emitted finish_reason").not.toBeNull();
    // Model invoked a tool.
    expect(result.toolCalls.length, "at least one tool_call was emitted").toBeGreaterThan(0);

    // Pre-fix, the model could NOT pick any tool by humfs_* name
    // because the schemas were null and the catalogue was empty.
    // Now humd merges the forager catalogue into the worker's MCP
    // server, so claude picks one of these.
    const calledNames = result.toolCalls.map((tc) => tc.name);
    expect(
      calledNames.some((n) => n === "read" || n === "humfs_read"),
      `expected a read-style tool call; got ${JSON.stringify(calledNames)}`,
    ).toBe(true);

    // The arguments parsed as JSON — i.e. the model emitted a real
    // schema-respecting call, not a hallucination.
    const args = result.toolCalls[0].arguments;
    expect(() => JSON.parse(args)).not.toThrow();
    const parsed = JSON.parse(args);
    expect(typeof parsed).toBe("object");

    // Content in the same response includes the file contents the
    // model just read. The worker bee's claude resolves the tool
    // call inline via the MCP bridge → humfs forager → real fs →
    // result back through the bridge into the model's continuation.
    // The hostname on this canary host is "canary".
    expect(result.content.toLowerCase()).toMatch(/canary|hostname/i);
  }, 120_000);

  test("plain prompt without tools still completes cleanly", async () => {
    skipIfDead();
    const result = await streamChat({
      model: "claude-haiku-4-5",
      messages: [{ role: "user", content: "Reply with exactly the four characters: ok!!" }],
    }, 60_000);
    expect(result.rawDoneAt).toBeGreaterThan(0);
    expect(result.finishReason).toBe("stop");
    expect(result.content.length).toBeGreaterThan(0);
  }, 90_000);
});

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

// The four humfs forager tools — each humd registers them on its
// claude-cli worker bridge under the same `mcp__hum__*` namespace
// OC sees. A successful round-trip per tool proves the merged
// catalogue lands in the worker's MCP server with valid schemas
// AND the worker bee's claude actually executes them via the
// bridge → forager → real filesystem.

// Each of these prompts ships NO tools[] from the asker side —
// humd merges the forager catalogue (humfs_read / humfs_do_code /
// humfs_do_noncode / humfs_bash) into the worker's MCP server
// automatically. So whatever claude sees IS what humfs provides;
// there's no asker write/edit/bash shadow to compete with. If a
// test fails, it means either the worker isn't seeing the merged
// catalogue or humfs isn't routing the call. Both are the actual
// product surface.

describe("live hum-openai-server: humfs forager tools", () => {
  // /tmp is in hum.json's fs.roots — humfs writes here.
  const tmpFile = `/tmp/hum-live-noncode-${Date.now()}.md`;
  const tmpFile2 = `/tmp/hum-live-docode-${Date.now()}.rs`;

  test("humfs_do_noncode: create a new file via humd's merged catalogue", async () => {
    skipIfDead();
    const marker = `WRITE_OK_${Date.now()}`;
    const result = await streamChat({
      model: "claude-haiku-4-5",
      messages: [{ role: "user", content:
        `Create the file ${tmpFile} containing exactly: ${marker}` }],
    }, 90_000);
    expect(result.rawDoneAt).toBeGreaterThan(0);
    expect(result.toolCalls.length).toBeGreaterThan(0);
    const calledNames = result.toolCalls.map((tc) => tc.name);
    expect(
      calledNames.some((n) => n === "humfs_do_noncode" || n === "humfs_do_code"),
      `expected humfs write-style tool call; got ${JSON.stringify(calledNames)}`,
    ).toBe(true);
    const { readFileSync, existsSync } = await import("node:fs");
    expect(existsSync(tmpFile), "humfs must materialize the file on disk").toBe(true);
    expect(readFileSync(tmpFile, "utf-8")).toContain(marker);
  }, 120_000);

  test("humfs_do_code: edit an existing file in place via humd's merged catalogue", async () => {
    skipIfDead();
    const { writeFileSync, readFileSync } = await import("node:fs");
    writeFileSync(tmpFile2, "fn the_target() {}\n// untouched comment\n");
    const result = await streamChat({
      model: "claude-haiku-4-5",
      messages: [{ role: "user", content:
        `In ${tmpFile2}, change the function name "the_target" to "the_replacement". Edit in place.` }],
    }, 90_000);
    expect(result.rawDoneAt).toBeGreaterThan(0);
    expect(result.toolCalls.length).toBeGreaterThan(0);
    const calledNames = result.toolCalls.map((tc) => tc.name);
    expect(
      calledNames.some((n) => n === "humfs_do_code"),
      `expected humfs_do_code tool call; got ${JSON.stringify(calledNames)}`,
    ).toBe(true);
    const after = readFileSync(tmpFile2, "utf-8");
    expect(after, "humfs_do_code must rewrite the symbol on disk").toContain("the_replacement");
    expect(after, "humfs_do_code must remove the old symbol").not.toContain("the_target");
    expect(after, "humfs_do_code must leave the unrelated comment intact").toContain("untouched comment");
  }, 120_000);

  test("humfs_bash: shell command output round-trips into model's content", async () => {
    skipIfDead();
    const sentinel = `BASH_${Date.now()}`;
    const result = await streamChat({
      model: "claude-haiku-4-5",
      messages: [{ role: "user", content:
        `Run the shell command: echo ${sentinel}. Then quote stdout in your reply.` }],
    }, 90_000);
    expect(result.rawDoneAt).toBeGreaterThan(0);
    expect(result.toolCalls.length).toBeGreaterThan(0);
    const calledNames = result.toolCalls.map((tc) => tc.name);
    expect(
      calledNames.some((n) => n === "humfs_bash"),
      `expected humfs_bash tool call; got ${JSON.stringify(calledNames)}`,
    ).toBe(true);
    expect(result.content).toContain(sentinel);
  }, 120_000);

  test("humfs_read: read an existing file via humd's merged catalogue", async () => {
    skipIfDead();
    const result = await streamChat({
      model: "claude-haiku-4-5",
      messages: [{ role: "user", content:
        `Read /etc/hostname and state the hostname.` }],
    }, 90_000);
    expect(result.rawDoneAt).toBeGreaterThan(0);
    expect(result.toolCalls.length).toBeGreaterThan(0);
    const calledNames = result.toolCalls.map((tc) => tc.name);
    expect(
      calledNames.some((n) => n === "humfs_read"),
      `expected humfs_read tool call; got ${JSON.stringify(calledNames)}`,
    ).toBe(true);
    expect(result.content.toLowerCase()).toMatch(/canary|hostname/i);
  }, 120_000);
});

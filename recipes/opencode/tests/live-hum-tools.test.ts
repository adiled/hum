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


// Regression — the OC TUI symptom: the worker bee resolves humfs_*
// tools internally via its MCP bridge, but the openai-server SSE
// stream still leaks the tool_use blocks to OC as OpenAI
// `tool_calls`. OC's claude SDK tries to execute the call against
// its own tool registry — which doesn't contain humfs_* (OC only
// knows its stock tools + the MCP servers in its own opencode.json,
// which does NOT register hum as MCP). OC marks the part
// `tool: invalid` and the UI stalls on an empty trailing message.
//
// What SHOULD reach OC:
//   - content text describing the action
//   - finish_reason=stop
//   - NO tool_calls block (the tool was already executed inside the
//     worker; OC doesn't need to and CAN'T re-execute it)
//
// What CURRENTLY reaches OC (the bug):
//   - tool_calls[{name:"humfs_do_code",...}] in the delta stream
//   - finish_reason="tool_calls" or mixed
//   - OC sees an unknown tool, marks the part invalid, stalls
//
// These tests assert the SHOULD-state. They will fail today.

// OC's integration uses the OpenAI Responses API (/v1/responses)
// via the `@ai-sdk/openai` provider. That's the standard interface
// that lets us emit forager-resolved tools as `mcp_call` hosted
// items — which OC's openai-responses.ts parser maps to
// tool-call + tool-result events tagged `providerExecuted: true`.
// Equivalent to the old clwnd-opencode plugin's flag, via a public
// OpenAI protocol, so OC stays a pure TUI.

interface ResponsesItem {
  type: string;
  id?: string;
  server_label?: string;
  name?: string;
  arguments?: string;
  output?: unknown;
  text?: string;
  status?: string;
  error?: unknown;
}
interface ResponsesEvent {
  type: string;
  item?: ResponsesItem;
  delta?: string;
  response?: { id?: string; usage?: Record<string, number> };
}

interface ResponsesCollected {
  events: ResponsesEvent[];
  items: ResponsesItem[];
  text: string;
  rawDoneAt: number;
}

async function streamResponses(body: Record<string, unknown>, timeoutMs: number): Promise<ResponsesCollected> {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), timeoutMs);
  try {
    const r = await fetch(`${BASE}/responses`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ stream: true, ...body }),
      signal: ctrl.signal,
    });
    if (!r.ok) throw new Error(`/responses returned ${r.status}: ${await r.text()}`);
    if (!r.body) throw new Error("no response body");
    const reader = r.body.getReader();
    const dec = new TextDecoder();
    let buf = "";
    const out: ResponsesCollected = { events: [], items: [], text: "", rawDoneAt: 0 };
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      buf += dec.decode(value, { stream: true });
      let nl: number;
      while ((nl = buf.indexOf("\n\n")) >= 0) {
        const block = buf.slice(0, nl);
        buf = buf.slice(nl + 2);
        const ev = parseSse(block);
        if (!ev) continue;
        if (ev === "[DONE]") { out.rawDoneAt = Date.now(); continue; }
        out.events.push(ev);
        if (ev.type === "response.output_text.delta" && typeof ev.delta === "string") {
          out.text += ev.delta;
        }
        if (ev.type === "response.output_item.done" && ev.item) {
          out.items.push(ev.item);
        }
      }
    }
    return out;
  } finally {
    clearTimeout(timer);
  }
}

function parseSse(block: string): ResponsesEvent | "[DONE]" | null {
  // OpenAI Responses uses `event: <name>\ndata: <json>` framing.
  // We accept either explicit event-line + data, or just data
  // with type embedded — our shim emits the second form.
  const lines = block.split("\n");
  let data = "";
  for (const line of lines) {
    if (line.startsWith("data: ")) data = line.slice(6);
  }
  if (!data) return null;
  if (data === "[DONE]") return "[DONE]";
  try { return JSON.parse(data) as ResponsesEvent; } catch { return null; }
}

describe("live hum-openai-server: /v1/responses (OpenAI Responses API)", () => {
  test("emits mcp_call items so OC renders forager tools as provider-executed", async () => {
    skipIfDead();
    const marker = `MARK_${Date.now()}`;
    const path = `/tmp/hum-responses-${Date.now()}.txt`;
    const result = await streamResponses({
      model: "claude-haiku-4-5",
      input: [{ role: "user", content: [{ type: "input_text", text: `Create ${path} with contents: ${marker}. Confirm when done.` }] }],
    }, 90_000);

    // Worker actually executed the tool against humfs.
    const { existsSync, readFileSync } = await import("node:fs");
    expect(existsSync(path), "worker resolved the tool against humfs").toBe(true);
    expect(readFileSync(path, "utf-8")).toContain(marker);

    // Stream closed cleanly.
    expect(result.rawDoneAt, "/responses SSE ended with [DONE]").toBeGreaterThan(0);

    // The stream MUST include an mcp_call item — that's the hosted-
    // tool shape OC's openai-responses parser recognizes as
    // provider-executed and tags providerExecuted=true on. Without
    // it, OC's tool runtime tries to re-execute and lands tool:invalid.
    const mcpCalls = result.items.filter((i) => i.type === "mcp_call");
    expect(
      mcpCalls.length,
      `expected at least one item.type="mcp_call" in /responses stream; got items: ` +
      `${result.items.map((i) => i.type).join(",")}`,
    ).toBeGreaterThan(0);

    const call = mcpCalls[0];
    expect(call.server_label, "mcp_call carries server_label (the forager hive)").toBeTruthy();
    expect(call.name, "mcp_call carries the tool name").toMatch(/humfs_/);
    expect(call.arguments, "mcp_call carries arguments JSON").toBeTruthy();
    expect(() => JSON.parse(String(call.arguments))).not.toThrow();

    const completed = result.events.find((e) => e.type === "response.completed");
    expect(completed, "stream ends with response.completed event").toBeTruthy();
  }, 120_000);

  test("plain prompt through /responses streams output_text deltas", async () => {
    skipIfDead();
    const result = await streamResponses({
      model: "claude-haiku-4-5",
      input: [{ role: "user", content: [{ type: "input_text", text: "Reply with exactly: hello-responses" }] }],
    }, 60_000);
    expect(result.rawDoneAt).toBeGreaterThan(0);
    expect(result.text.toLowerCase()).toContain("hello-responses");
    const completed = result.events.find((e) => e.type === "response.completed");
    expect(completed).toBeTruthy();
  }, 90_000);

  // One mcp_call assertion per forager tool — each humd registers
  // them on its worker bridge under the same `mcp__hum__*` namespace
  // OC sees. The asker ships no tools[]; humd auto-merges the
  // forager catalogue into the worker's MCP server, so the only
  // tools claude can call are the humfs_* surfaces. Each side
  // effect (file on disk, output in content) confirms the call
  // ran end-to-end.

  test("humfs_read via mcp_call: hostname round-trips through forager", async () => {
    skipIfDead();
    const result = await streamResponses({
      model: "claude-haiku-4-5",
      input: [{ role: "user", content: [{ type: "input_text", text: `Read /etc/hostname and state the hostname.` }] }],
    }, 90_000);
    const calls = result.items.filter((i) => i.type === "mcp_call" && i.name === "humfs_read");
    expect(calls.length).toBeGreaterThan(0);
    expect(JSON.stringify(calls[0].output)).toMatch(/canary/);
  }, 120_000);

  test("humfs_do_noncode via mcp_call: file materializes on disk", async () => {
    skipIfDead();
    const tmpFile = `/tmp/hum-r-noncode-${Date.now()}.md`;
    const marker = `WRITE_OK_${Date.now()}`;
    const result = await streamResponses({
      model: "claude-haiku-4-5",
      input: [{ role: "user", content: [{ type: "input_text", text: `Create ${tmpFile} containing exactly: ${marker}` }] }],
    }, 90_000);
    const calls = result.items.filter((i) =>
      i.type === "mcp_call" && (i.name === "humfs_do_noncode" || i.name === "humfs_do_code")
    );
    expect(calls.length, `expected a humfs write tool call; got: ${result.items.map((i) => i.name).join(",")}`).toBeGreaterThan(0);
    const { readFileSync, existsSync } = await import("node:fs");
    expect(existsSync(tmpFile)).toBe(true);
    expect(readFileSync(tmpFile, "utf-8")).toContain(marker);
  }, 120_000);

  test("humfs_do_code via mcp_call: symbol rewrite in place", async () => {
    skipIfDead();
    const tmpFile = `/tmp/hum-r-docode-${Date.now()}.rs`;
    const { writeFileSync, readFileSync } = await import("node:fs");
    writeFileSync(tmpFile, "fn the_target() {}\n// untouched comment\n");
    const result = await streamResponses({
      model: "claude-haiku-4-5",
      input: [{ role: "user", content: [{ type: "input_text", text: `In ${tmpFile}, rename the_target to the_replacement. Edit in place.` }] }],
    }, 90_000);
    const calls = result.items.filter((i) => i.type === "mcp_call" && i.name === "humfs_do_code");
    expect(calls.length).toBeGreaterThan(0);
    const after = readFileSync(tmpFile, "utf-8");
    expect(after).toContain("the_replacement");
    expect(after).not.toContain("the_target");
    expect(after).toContain("untouched comment");
  }, 120_000);

  test("humfs_bash via mcp_call: stdout surfaces in mcp_call output", async () => {
    skipIfDead();
    const sentinel = `BASH_${Date.now()}`;
    const result = await streamResponses({
      model: "claude-haiku-4-5",
      input: [{ role: "user", content: [{ type: "input_text", text: `Run the shell command: echo ${sentinel}` }] }],
    }, 90_000);
    const calls = result.items.filter((i) => i.type === "mcp_call" && i.name === "humfs_bash");
    expect(calls.length).toBeGreaterThan(0);
    expect(JSON.stringify(calls[0].output)).toContain(sentinel);
  }, 120_000);
});

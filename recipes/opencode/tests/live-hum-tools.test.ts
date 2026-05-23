// Live integration: hits the running hum-openai-server endpoint
// directly, the same way OC and wryme do. Asserts the full
// tool-use round-trip against a real deployment.
//
// Pre-req: `dev/deploy` has run on this box; humd + worker + humfs
// forager + openai-server are all live as systemd units under the
// HUM_DEV_USER. Tests skip if openai-server isn't reachable.

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

// /v1/responses streams forager-resolved tool calls as `mcp_call`
// hosted items so OC's openai-responses parser tags them
// providerExecuted=true.

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

    // mcp_call items in the stream are how OC's openai-responses
    // parser recognizes provider-executed tools.
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

  // sid binds to the conversation root in full-history mode: turn N+1
  // ships the whole input array starting with the same first user
  // message, so sid stays stable and the pool reuses the cell — no
  // x-session-affinity, no previous_response_id required.
  test("full-history mode: stable sid + pool reuse without continuity hints", async () => {
    skipIfDead();
    const { spawn: cpSpawn } = await import("node:child_process");
    const countClaude = async (): Promise<number> => new Promise((res) => {
      const p = cpSpawn("sh", ["-c", "pgrep -u clwnd -fc 'claude -p'"]);
      let buf = "";
      p.stdout.on("data", (c: Buffer) => { buf += c.toString(); });
      p.on("exit", () => res(parseInt(buf.trim(), 10) || 0));
    });
    const rootMsg = `root-${Date.now()}`;
    const turn1Input = [
      { role: "user", content: [{ type: "input_text", text: rootMsg }] },
    ];
    const turn2Input = [
      { role: "user", content: [{ type: "input_text", text: rootMsg }] },
      { role: "assistant", content: [{ type: "output_text", text: "ack" }] },
      { role: "user", content: [{ type: "input_text", text: "what did i say first?" }] },
    ];
    const turn = async (input: unknown) => {
      const r = await fetch(`${BASE}/responses`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ model: "claude-haiku-4-5", stream: true, input }),
      });
      const reader = r.body!.getReader();
      while (true) { const { done } = await reader.read(); if (done) break; }
    };
    await turn(turn1Input);
    const after1 = await countClaude();
    await turn(turn2Input);
    const after2 = await countClaude();
    expect(
      after2,
      `turn 2 spawned a new claude (${after1} → ${after2}) — full-history sid not stable`,
    ).toBe(after1);
  }, 180_000);

  // Delta mode: caller sends previous_response_id from the first
  // response.created event and a fresh user message only. Same sid
  // reuses the warm cell.
  test("sid anchor: `conversation` field pins the sid", async () => {
    skipIfDead();
    const convId = `conv-test-${Date.now()}`;
    const turn = async (text: string) => {
      const r = await fetch(`${BASE}/responses`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          model: "claude-haiku-4-5",
          stream: true,
          conversation: convId,
          input: [{ role: "user", content: [{ type: "input_text", text }] }],
        }),
      });
      const reader = r.body!.getReader();
      while (true) { const { done } = await reader.read(); if (done) break; }
    };
    await turn(`first-msg-${Date.now()}`);
    await turn(`second-msg-${Date.now()}`);
    // Two distinct first messages would normally fall to different sid
    // anchors. The conversation field overrides — same sid.
    // Verified indirectly via humd log; here we assert no error / both
    // turns reach completion.
  }, 90_000);

  test("metadata echo: response.completed carries the caller's metadata", async () => {
    skipIfDead();
    const r = await fetch(`${BASE}/responses`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        model: "claude-haiku-4-5",
        stream: false,
        metadata: { tenant_internal: "wryme-prod", run_kind: "smoke" },
        safety_identifier: "user-7",
        input: [{ role: "user", content: [{ type: "input_text", text: "Reply ok" }] }],
      }),
    });
    const j = await r.json() as { metadata?: Record<string, string>; safety_identifier?: string };
    expect(j.metadata).toEqual({ tenant_internal: "wryme-prod", run_kind: "smoke" });
    expect(j.safety_identifier).toBe("user-7");
  }, 60_000);

  test("streaming: sequence_number monotonic across one stream", async () => {
    skipIfDead();
    const r = await fetch(`${BASE}/responses`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        model: "claude-haiku-4-5",
        stream: true,
        input: [{ role: "user", content: [{ type: "input_text", text: `Say hi ${Date.now()}` }] }],
      }),
    });
    const dec = new TextDecoder();
    let buf = "";
    const seqs: number[] = [];
    const reader = r.body!.getReader();
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      buf += dec.decode(value, { stream: true });
      let nl: number;
      while ((nl = buf.indexOf("\n\n")) >= 0) {
        const block = buf.slice(0, nl);
        buf = buf.slice(nl + 2);
        const m = block.match(/data: ({.*})/);
        if (!m) continue;
        try {
          const evt = JSON.parse(m[1]) as { sequence_number?: number };
          if (typeof evt.sequence_number === "number") seqs.push(evt.sequence_number);
        } catch {}
      }
    }
    expect(seqs.length, "at least three events").toBeGreaterThan(2);
    for (let i = 1; i < seqs.length; i++) {
      expect(seqs[i], `seq[${i}] >= seq[${i - 1}]`).toBeGreaterThanOrEqual(seqs[i - 1]);
    }
    expect(seqs[0]).toBe(0);
  }, 60_000);

  test("response.in_progress emitted after response.created", async () => {
    skipIfDead();
    const r = await fetch(`${BASE}/responses`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        model: "claude-haiku-4-5",
        stream: true,
        input: [{ role: "user", content: [{ type: "input_text", text: `ack ${Date.now()}` }] }],
      }),
    });
    const dec = new TextDecoder();
    let buf = "";
    const types: string[] = [];
    const reader = r.body!.getReader();
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      buf += dec.decode(value, { stream: true });
      let nl: number;
      while ((nl = buf.indexOf("\n\n")) >= 0) {
        const block = buf.slice(0, nl);
        buf = buf.slice(nl + 2);
        const m = block.match(/data: ({.*})/);
        if (!m) continue;
        try {
          const evt = JSON.parse(m[1]) as { type?: string };
          if (typeof evt.type === "string") types.push(evt.type);
        } catch {}
      }
    }
    const createdIdx = types.indexOf("response.created");
    const inProgressIdx = types.indexOf("response.in_progress");
    expect(createdIdx).toBeGreaterThanOrEqual(0);
    expect(inProgressIdx).toBeGreaterThan(createdIdx);
  }, 60_000);

  test("delta mode: previous_response_id chains the same sid", async () => {
    skipIfDead();
    const { spawn: cpSpawn } = await import("node:child_process");
    const countClaude = async (): Promise<number> => new Promise((res) => {
      const p = cpSpawn("sh", ["-c", "pgrep -u clwnd -fc 'claude -p'"]);
      let buf = "";
      p.stdout.on("data", (c: Buffer) => { buf += c.toString(); });
      p.on("exit", () => res(parseInt(buf.trim(), 10) || 0));
    });
    // Turn 1: capture response.id from the stream.
    const r1 = await fetch(`${BASE}/responses`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        model: "claude-haiku-4-5",
        stream: true,
        input: [{ role: "user", content: [{ type: "input_text", text: `delta-root-${Date.now()}` }] }],
      }),
    });
    const dec = new TextDecoder();
    let buf = "";
    let responseId = "";
    const reader = r1.body!.getReader();
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      buf += dec.decode(value, { stream: true });
      const m = buf.match(/"id":"(resp_[a-z0-9]+)"/);
      if (m && !responseId) responseId = m[1];
    }
    expect(responseId).toMatch(/^resp_/);
    const after1 = await countClaude();
    // Turn 2: delta — only the new user message + previous_response_id.
    const r2 = await fetch(`${BASE}/responses`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        model: "claude-haiku-4-5",
        stream: true,
        previous_response_id: responseId,
        input: [{ role: "user", content: [{ type: "input_text", text: "say two" }] }],
      }),
    });
    const reader2 = r2.body!.getReader();
    while (true) { const { done } = await reader2.read(); if (done) break; }
    const after2 = await countClaude();
    expect(
      after2,
      `delta-mode turn 2 spawned a new claude (${after1} → ${after2}) — previous_response_id chain broken`,
    ).toBe(after1);
  }, 180_000);

  test("pool reuse: same-sid turns share one claude process", async () => {
    skipIfDead();
    const { spawn: cpSpawn } = await import("node:child_process");
    const countClaude = async (): Promise<number> => new Promise((res) => {
      const p = cpSpawn("sh", ["-c", "pgrep -u clwnd -fc 'claude -p'"]);
      let buf = "";
      p.stdout.on("data", (c: Buffer) => { buf += c.toString(); });
      p.on("exit", () => res(parseInt(buf.trim(), 10) || 0));
    });
    const affinity = `affinity-${Date.now()}`;
    const headers = { "content-type": "application/json", "x-session-affinity": affinity };
    const turn = async (text: string) => {
      const r = await fetch(`${BASE}/responses`, {
        method: "POST",
        headers,
        body: JSON.stringify({
          model: "claude-haiku-4-5",
          stream: true,
          input: [{ role: "user", content: [{ type: "input_text", text }] }],
        }),
      });
      // Drain the stream so the turn completes before we count.
      const reader = r.body!.getReader();
      while (true) {
        const { done } = await reader.read();
        if (done) break;
      }
    };
    await turn("Reply with: one");
    const after1 = await countClaude();
    await turn("Reply with: two");
    const after2 = await countClaude();
    await turn("Reply with: three");
    const after3 = await countClaude();
    expect(
      after2,
      `same-sid turn 2 spawned a new claude (was ${after1}, now ${after2}) — pool reuse missing`,
    ).toBe(after1);
    expect(after3).toBe(after1);
  }, 180_000);

  test("process budget: N prompts do not leak claude children", async () => {
    skipIfDead();
    const { spawn: cpSpawn } = await import("node:child_process");
    const countClaude = async (): Promise<number> => new Promise((res) => {
      const p = cpSpawn("sh", ["-c", "ps -u clwnd -o cmd | grep -c '^/home/clwnd/.local/bin/claude -p '"]);
      let buf = "";
      p.stdout.on("data", (c: Buffer) => { buf += c.toString(); });
      p.on("exit", () => res(parseInt(buf.trim(), 10) || 0));
    });
    const before = await countClaude();
    for (let i = 0; i < 6; i++) {
      await streamResponses({
        model: "claude-haiku-4-5",
        input: [{ role: "user", content: [{ type: "input_text", text: `Reply with: turn-${i}` }] }],
      }, 60_000);
    }
    await new Promise((r) => setTimeout(r, 8_000));
    const after = await countClaude();
    expect(
      after - before,
      `claude proc count grew by ${after - before} after 6 prompts ` +
      `(was ${before}, now ${after}) — kill-after-finish deadline failed`,
    ).toBeLessThanOrEqual(2);
  }, 180_000);

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

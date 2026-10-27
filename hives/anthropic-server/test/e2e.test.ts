// End to end: the official Anthropic SDK, as a client that knows nothing of
// hum, against this hive's server under Node, a real humd, and a scripted
// nurse (nurse-bee.ts).
//
// What it holds to: a request without `stream` gets one message as JSON;
// a stream carries words and sealed thinking; a turn carries the turns
// before it as its graft and keeps its sigil; images, documents and a
// system of blocks reach the nurse; how the request asks the bloom to grow
// becomes the prompt's; a caller's tool comes back as tool_use, one or
// several at once, and its result grows the same bloom on; a result the
// server no longer holds grows on from a graft; a caller that hangs up
// cancels its bloom; failures carry their status; and the key holds.

import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import Anthropic from "@anthropic-ai/sdk";
import { hum, type Hum } from "./hum.ts";

const MODEL = "claude-lab";
const KEY = "sk-lab";
const PNG = "iVBORw0KGgo=";
const PDF = "JVBERi0xLjQK";

let h: Hum;
let ai: Anthropic;

const tag = () => Math.random().toString(36).slice(2, 8);
const readFile: Anthropic.Tool = { name: "readFile", description: "Read a file.", input_schema: { type: "object", properties: { path: { type: "string" } } } };
const said = (p: any) => (p.content ?? []).filter((x: any) => x.kind === "text").map((x: any) => x.text).join(" ");
const promptSaying = async (words: string) => (await h.heard()).prompts.find((p) => said(p).includes(words));

beforeAll(async () => {
  h = await hum({ env: { ANTHROPIC_SERVER_API_KEY: KEY } });
  ai = new Anthropic({ baseURL: h.base, apiKey: KEY, maxRetries: 0, timeout: 20_000 });
}, 120_000);

afterAll(async () => {
  await h?.close();
});

describe("messages", () => {
  test("a request without stream gets one message, as JSON", async () => {
    const r = await ai.messages.create({ model: MODEL, max_tokens: 64, messages: [{ role: "user", content: `hi ${tag()}` }] });
    expect(r.type).toBe("message");
    expect(r.role).toBe("assistant");
    expect(r.content[0].type === "text" && r.content[0].text).toStartWith("echo: hi");
    expect(r.stop_reason).toBe("end_turn");
    expect(r.stop_sequence).toBeNull();
    expect(r.usage).toMatchObject({ input_tokens: 11, output_tokens: 7, cache_read_input_tokens: 4 });
  });

  test("a stream carries words, and thinking sealed by its signature", async () => {
    const stream = ai.messages.stream({ model: MODEL, max_tokens: 64, messages: [{ role: "user", content: `THINK ${tag()}` }] });
    const types: string[] = [];
    stream.on("streamEvent", (e) => types.push(e.type));
    const m = await stream.finalMessage();
    expect(types[0]).toBe("message_start");
    expect(types.at(-1)).toBe("message_stop");
    expect(m.content).toEqual([
      { type: "thinking", thinking: "pondering", signature: "sig-lab" },
      { type: "text", text: "thought it through" },
    ]);
    expect(m.stop_reason).toBe("end_turn");
    expect(m.usage.output_tokens).toBe(7);
  });

  test("a turn carries the turns before it as its graft, on the same sigil", async () => {
    const t = tag();
    const system: Anthropic.TextBlockParam[] = [{ type: "text", text: "be terse", cache_control: { type: "ephemeral" } }];
    const first: Anthropic.MessageParam[] = [{ role: "user", content: `first ${t}` }];
    const a = await ai.messages.create({ model: MODEL, max_tokens: 64, system, messages: first });
    await ai.messages.create({
      model: MODEL,
      max_tokens: 64,
      system,
      messages: [...first, { role: "assistant", content: a.content }, { role: "user", content: [{ type: "text", text: `second ${t}` }] }],
    });
    const one = (await promptSaying(`first ${t}`))!;
    const two = (await promptSaying(`second ${t}`))!;
    expect(two.systemPrompt).toBe("be terse");
    expect(two.content).toEqual([{ kind: "text", text: `second ${t}` }]);
    expect(two.graft).toEqual([{ content: [{ kind: "text", text: `first ${t}` }], grew: [{ kind: "text", text: `echo: first ${t}` }] }]);
    expect(two.sid).toBe(one.sid);
  });

  test("images and documents reach the nurse as pollen", async () => {
    const t = tag();
    await ai.messages.create({
      model: MODEL,
      max_tokens: 64,
      messages: [{
        role: "user",
        content: [
          { type: "text", text: `look ${t}` },
          { type: "image", source: { type: "base64", media_type: "image/png", data: PNG } },
          { type: "image", source: { type: "url", url: "https://example.com/bee.jpg" } },
          { type: "document", source: { type: "base64", media_type: "application/pdf", data: PDF } },
          { type: "document", source: { type: "url", url: "https://example.com/hive.pdf" } },
          { type: "document", source: { type: "text", media_type: "text/plain", data: "honey" } },
          { type: "document", source: { type: "content", content: [{ type: "text", text: "comb" }] } },
        ],
      }],
    });
    expect((await promptSaying(`look ${t}`))!.content).toEqual([
      { kind: "text", text: `look ${t}` },
      { kind: "pollen", grain: { kind: "image", mediaType: "image/png", data: PNG } },
      { kind: "pollen", grain: { kind: "image", mediaType: "image/*", url: "https://example.com/bee.jpg" } },
      { kind: "pollen", grain: { kind: "file", mediaType: "application/pdf", data: PDF } },
      { kind: "pollen", grain: { kind: "file", mediaType: "application/pdf", url: "https://example.com/hive.pdf" } },
      { kind: "pollen", grain: { kind: "file", mediaType: "text/plain", data: Buffer.from("honey").toString("base64") } },
      { kind: "text", text: "comb" },
    ]);
  });

  test("how the request asks its bloom to grow becomes the prompt's", async () => {
    const t = tag();
    await ai.messages.create({
      model: MODEL,
      max_tokens: 33,
      temperature: 0.2,
      top_p: 0.5,
      top_k: 7,
      stop_sequences: ["END"],
      thinking: { type: "enabled", budget_tokens: 1024 },
      tools: [readFile],
      tool_choice: { type: "tool", name: "readFile", disable_parallel_tool_use: true },
      messages: [{ role: "user", content: `grow ${t}` }],
    });
    expect(await promptSaying(`grow ${t}`)).toMatchObject({
      season: 33,
      temperature: 0.2,
      prune: { p: 0.5, k: 7 },
      frost: ["END"],
      sepal: { budget: 1024 },
      tendrils: { reach: "readFile", parallel: false },
    });
  });

  test("a conversation ending in the assistant's own words grows on from them", async () => {
    const t = tag();
    const before = (await h.heard()).prompts.length;
    const r = await ai.messages.create({ model: MODEL, max_tokens: 64, messages: [{ role: "user", content: `tell ${t}` }, { role: "assistant", content: "Once upon" }] });
    expect(r.content).toEqual([{ type: "text", text: "grew on" }]);
    const prompt = (await h.heard()).prompts[before];
    expect(prompt.content).toEqual([]);
    expect(prompt.graft).toEqual([{ content: [{ kind: "text", text: `tell ${t}` }], grew: [{ kind: "text", text: "Once upon" }] }]);
  });

  test("a bloom cut short says max_tokens", async () => {
    const r = await ai.messages.create({ model: MODEL, max_tokens: 64, messages: [{ role: "user", content: `LEN ${tag()}` }] });
    expect(r.stop_reason).toBe("max_tokens");
  });

  test("beta messages are the same messages", async () => {
    const r = await ai.beta.messages.create({ model: MODEL, max_tokens: 64, messages: [{ role: "user", content: `beta ${tag()}` }] });
    expect(r.content[0].type === "text" && r.content[0].text).toStartWith("echo: beta");
  });
});

describe("tools", () => {
  test("a caller's tool comes back as tool_use, and its result grows the same bloom on", async () => {
    const t = tag();
    const messages: Anthropic.MessageParam[] = [{ role: "user", content: `ONE ${t}` }];
    const asked = await ai.messages.create({ model: MODEL, max_tokens: 64, tools: [readFile], messages });
    expect(asked.stop_reason).toBe("tool_use");
    const use = asked.content.find((b) => b.type === "tool_use") as Anthropic.ToolUseBlock;
    expect(use).toMatchObject({ name: "readFile", input: { path: "x" } });
    expect(asked.content[0]).toEqual({ type: "text", text: "let me look" });
    const before = (await h.heard()).prompts.length;
    const answered = await ai.messages.create({
      model: MODEL,
      max_tokens: 64,
      tools: [readFile],
      messages: [...messages, { role: "assistant", content: asked.content }, { role: "user", content: [{ type: "tool_result", tool_use_id: use.id, content: "X-CONTENT" }] }],
    });
    expect(answered.content).toEqual([{ type: "text", text: "after tools: X-CONTENT" }]);
    expect(answered.stop_reason).toBe("end_turn");
    const heard = await h.heard();
    expect(heard.prompts.length).toBe(before);
    const result = heard.results.find((r) => r.callId === use.id);
    expect(result).toMatchObject({ output: "X-CONTENT", sid: (await promptSaying(`ONE ${t}`))!.sid });
  });

  test("tools reached at once stream back together, and so do their results", async () => {
    const messages: Anthropic.MessageParam[] = [{ role: "user", content: `PAR ${tag()}` }];
    const asked = await ai.messages.stream({ model: MODEL, max_tokens: 64, tools: [readFile], messages }).finalMessage();
    expect(asked.stop_reason).toBe("tool_use");
    const uses = asked.content.filter((b) => b.type === "tool_use") as Anthropic.ToolUseBlock[];
    expect(uses.map((u) => u.input)).toEqual([{ path: "a" }, { path: "b" }]);
    const answered = await ai.messages.stream({
      model: MODEL,
      max_tokens: 64,
      tools: [readFile],
      messages: [
        ...messages,
        { role: "assistant", content: asked.content },
        { role: "user", content: uses.map((u) => ({ type: "tool_result" as const, tool_use_id: u.id, content: [{ type: "text" as const, text: `got ${(u.input as { path: string }).path}` }] })) },
      ],
    }).finalMessage();
    const text = answered.content[0].type === "text" ? answered.content[0].text : "";
    expect(text).toContain("got a");
    expect(text).toContain("got b");
  });

  test("a tool result the server no longer holds grows on from a graft", async () => {
    const t = tag();
    const before = (await h.heard()).prompts.length;
    const r = await ai.messages.create({
      model: MODEL,
      max_tokens: 64,
      tools: [readFile],
      messages: [
        { role: "user", content: `ONE ${t}` },
        { role: "assistant", content: [{ type: "tool_use", id: "toolu_old", name: "readFile", input: { path: "x" } }] },
        { role: "user", content: [{ type: "tool_result", tool_use_id: "toolu_old", content: "OLD-ANSWER", is_error: true }] },
      ],
    });
    expect(r.content).toEqual([{ type: "text", text: "after tools: OLD-ANSWER" }]);
    const prompt = (await h.heard()).prompts[before];
    expect(prompt.content ?? []).toEqual([]);
    expect(prompt.graft).toEqual([{
      content: [{ kind: "text", text: `ONE ${t}` }],
      grew: [
        { kind: "tendril", callId: "toolu_old", name: "readFile", args: { path: "x" } },
        { kind: "grasp", callId: "toolu_old", result: "OLD-ANSWER", isError: true },
      ],
    }]);
  });
});

describe("the wire", () => {
  test("a caller that hangs up cancels its bloom", async () => {
    const t = tag();
    const aborter = new AbortController();
    const stream = await ai.messages.create({ model: MODEL, max_tokens: 64, stream: true, messages: [{ role: "user", content: `SLOW ${t}` }] }, { signal: aborter.signal });
    let seen = 0;
    try {
      for await (const _ of stream) if (++seen === 4) aborter.abort();
    } catch {}
    const sid = (await promptSaying(`SLOW ${t}`))!.sid;
    for (let i = 0; i < 40 && !(await h.heard()).cancels.includes(sid); i++) await new Promise((r) => setTimeout(r, 100));
    expect((await h.heard()).cancels).toContain(sid);
    const quiet = new AbortController();
    setTimeout(() => quiet.abort(), 600);
    await ai.messages.create({ model: MODEL, max_tokens: 64, messages: [{ role: "user", content: `SLOW quiet ${t}` }] }, { signal: quiet.signal }).catch(() => {});
    const quietSid = (await promptSaying(`SLOW quiet ${t}`))!.sid;
    for (let i = 0; i < 40 && !(await h.heard()).cancels.includes(quietSid); i++) await new Promise((r) => setTimeout(r, 100));
    expect((await h.heard()).cancels).toContain(quietSid);
  }, 20_000);

  test("failures carry their status", async () => {
    const unknown = await ai.messages.create({ model: "no-such-model", max_tokens: 64, messages: [{ role: "user", content: "hi" }] }).catch((e) => e);
    expect(unknown).toBeInstanceOf(Anthropic.NotFoundError);
    const streamed = await ai.messages.stream({ model: "no-such-model", max_tokens: 64, messages: [{ role: "user", content: "hi" }] }).finalMessage().catch((e) => e);
    expect(streamed).toBeInstanceOf(Anthropic.NotFoundError);
    const broke = await ai.messages.create({ model: MODEL, max_tokens: 64, messages: [{ role: "user", content: `FAIL ${tag()}` }] }).catch((e) => e);
    expect(broke).toBeInstanceOf(Anthropic.InternalServerError);
    expect((broke as InstanceType<typeof Anthropic.APIError>).status).toBe(502);
    const empty = await ai.messages.create({ model: MODEL, max_tokens: 64, messages: [] }).catch((e) => e);
    expect(empty).toBeInstanceOf(Anthropic.BadRequestError);
  }, 30_000);

  test("a request without the key, or with a wrong one, is turned away", async () => {
    const wrong = await new Anthropic({ baseURL: h.base, apiKey: "sk-nope", maxRetries: 0 }).messages.create({ model: MODEL, max_tokens: 8, messages: [{ role: "user", content: "hi" }] }).catch((e) => e);
    expect(wrong).toBeInstanceOf(Anthropic.AuthenticationError);
    expect((await fetch(`${h.base}/v1/messages`, { method: "POST", body: "{}" })).status).toBe(401);
  });

  test("usage counts what the tenant took", async () => {
    const r = await fetch(`${h.base}/v1/usage`, { headers: { "x-api-key": KEY } });
    const body = (await r.json()) as { tenant: string; usage: { requests: number; inputTokens: number } };
    expect(body.tenant).toBe("default");
    expect(body.usage.requests).toBeGreaterThan(5);
    expect(body.usage.inputTokens).toBeGreaterThan(0);
  });
});

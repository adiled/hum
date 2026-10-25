// Live: real claude, through Claude Code and this branch's claude-cli
// nurse, behind this hive, driven by the official OpenAI SDK.
//
// It runs only with HUM_LIVE_CLAUDE=1, since it signs claude in the way
// the person's own Claude Code does and spends their plan: build the nurse
// first (`cargo build` in hives/claude-cli). HUM_LIVE_MODEL picks the model,
// claude-haiku-4-5 by default. hum lives under a fresh directory as in the
// end-to-end tests; only the nurse sees the person's home, where claude
// keeps its sign-in and, under ~/.claude/projects, the transcripts of the
// nest the test made, which it removes once everything it started has
// stopped, and only the nurse has the person's user, as claude's sign-in
// asks.
//
// What it holds to: answers streamed and not, with usage; a conversation
// remembered turn to turn on one sigil; a conversation the nurse never saw,
// tool calls and all, grafted so claude knows it; a caller's tools, one or
// several, answered and carried on; a tool the server runs, hidden from
// chat completions and shown in a response; a JSON shape and a system
// prompt honoured; an image seen; the Responses API's function calls; a
// hang-up that stops claude; two conversations at once kept apart; and
// what claude cannot do refused plainly.

import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { realpathSync, rmSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import OpenAI from "openai";
import { hum, type Hum } from "./hum.ts";
import { upstream, type Upstream } from "./upstream.ts";

const live = process.env.HUM_LIVE_CLAUDE === "1";
const MODEL = process.env.HUM_LIVE_MODEL ?? "claude-haiku-4-5";
const NURSE = resolve(import.meta.dir, "../../claude-cli/target/debug/claude-cli-nurse");
const RED = "iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAIAAACQkWg2AAAAF0lEQVR4nGP4z8BAEiJN9aiGUQ1DSgMAkPn/Afnh+ngAAAAASUVORK5CYII=";
const SLOW = 180_000;

let up: Upstream;
let h: Hum;
let nurse: ChildProcess;
let nurseLog = "";
let ai: OpenAI;

const tag = () => Math.random().toString(36).slice(2, 7).toUpperCase();
const weather = { type: "function" as const, function: { name: "get_weather", description: "The weather in a city right now.", parameters: { type: "object", properties: { city: { type: "string" } }, required: ["city"] } } };
const forecast: Record<string, string> = { paris: "Sunny, 31C", tokyo: "Rain, 18C" };

async function untilAnswered(messages: OpenAI.ChatCompletionMessageParam[], tools: OpenAI.ChatCompletionTool[]): Promise<{ text: string; asked: string[] }> {
  const asked: string[] = [];
  for (let round = 0; round < 5; round++) {
    const r = await ai.chat.completions.create({ model: MODEL, tools, messages });
    const m = r.choices[0].message;
    if (!m.tool_calls || m.tool_calls.length === 0) return { text: m.content ?? "", asked };
    expect(r.choices[0].finish_reason).toBe("tool_calls");
    messages = [...messages, m];
    for (const c of m.tool_calls) {
      if (c.type !== "function") continue;
      const city = String(JSON.parse(c.function.arguments || "{}").city ?? "").toLowerCase();
      asked.push(`${c.function.name}:${city}`);
      messages.push({ role: "tool", tool_call_id: c.id, content: forecast[city] ?? "unknown city" });
    }
  }
  throw new Error("still calling tools after five rounds");
}

function nest(): string {
  return realpathSync(join(h.env.XDG_STATE_HOME, "hum", "hives", "claude-cli", "nest"));
}

(live ? describe : describe.skip)(`live claude (${MODEL})`, () => {
  beforeAll(async () => {
    up = await upstream();
    h = await hum(up.url, { keys: ["live"], hives: ["claude-cli"] });
    const own = Object.fromEntries(["USER", "LOGNAME", "SHELL", "TMPDIR", "LANG", "TERM"].flatMap((k) => (process.env[k] ? [[k, process.env[k]!]] : [])));
    nurse = spawn(NURSE, [], {
      env: { ...h.env, ...own, HOME: homedir(), NO_COLOR: "1", CLAUDE_MODELS: MODEL, HUM_LOG_LEVEL: "info,hum_nest=trace,claude_cli=trace" },
      stdio: ["ignore", "pipe", "pipe"],
    });
    const keep = (b: Buffer) => (nurseLog += String(b).replace(/\x1b\[[0-9;]*m/g, ""));
    nurse.stdout?.on("data", keep);
    nurse.stderr?.on("data", keep);
    ai = new OpenAI({ baseURL: h.base, apiKey: h.keys.live, maxRetries: 0, timeout: SLOW });
    const deadline = Date.now() + 60_000;
    while (Date.now() < deadline) {
      const ids = (await ai.models.list()).data.map((m) => m.id);
      if (ids.includes(MODEL)) return;
      await new Promise((r) => setTimeout(r, 200));
    }
    throw new Error(`the claude nurse never offered ${MODEL}\n${nurseLog}`);
  }, 120_000);

  afterAll(async () => {
    const transcripts = join(homedir(), ".claude", "projects", nest().replace(/[^A-Za-z0-9]/g, "-").replace(/-+$/, ""));
    nurse?.kill("SIGKILL");
    await h?.close();
    await up?.close();
    await new Promise((r) => setTimeout(r, 1000));
    rmSync(transcripts, { recursive: true, force: true });
  }, 30_000);

  test("the claude nurse is listed", async () => {
    expect((await ai.models.retrieve(MODEL)).owned_by).toBe("claude-cli");
  });

  test("answers, whole and streamed, with usage", async () => {
    const whole = await ai.chat.completions.create({ model: MODEL, messages: [{ role: "user", content: "Reply with exactly this and nothing else: PONG-ONE" }] });
    expect(whole.choices[0].message.content).toContain("PONG-ONE");
    expect(whole.usage!.prompt_tokens).toBeGreaterThan(0);
    const stream = await ai.chat.completions.create({ model: MODEL, stream: true, stream_options: { include_usage: true }, messages: [{ role: "user", content: "Reply with exactly this and nothing else: PONG-TWO" }] });
    let text = "";
    let total = 0;
    for await (const c of stream) {
      text += c.choices[0]?.delta?.content ?? "";
      total = c.usage?.total_tokens ?? total;
    }
    expect(text).toContain("PONG-TWO");
    expect(total).toBeGreaterThan(0);
  }, SLOW);

  test("a conversation is remembered turn to turn, on one sigil", async () => {
    const word = `TANGERINE-${tag()}`;
    const first: OpenAI.ChatCompletionMessageParam[] = [{ role: "user", content: `Remember this codeword: ${word}. Reply only with OK.` }];
    const a = await ai.chat.completions.create({ model: MODEL, messages: first });
    const before = nurseLog.length;
    const b = await ai.chat.completions.create({
      model: MODEL,
      messages: [...first, { role: "assistant", content: a.choices[0].message.content ?? "" }, { role: "user", content: "What was the codeword? Reply with only the codeword." }],
    });
    expect(b.choices[0].message.content).toContain(word);
    expect(nurseLog.slice(before)).toMatch(/nurse\.cell\.reused|grafted=0/);
  }, SLOW);

  test("a conversation claude never saw is grafted, and claude knows it", async () => {
    const word = `VERMILION-${tag()}`;
    const before = nurseLog.length;
    const r = await ai.chat.completions.create({
      model: MODEL,
      messages: [
        { role: "user", content: `My favourite colour is ${word}.` },
        { role: "assistant", content: "Noted, I will remember your favourite colour." },
        { role: "user", content: "What is my favourite colour? Reply with only it." },
      ],
    });
    expect(r.choices[0].message.content).toContain(word);
    expect(nurseLog.slice(before)).toContain("grafted=1");
  }, SLOW);

  test("grafted tool calls and their answers read as claude's own", async () => {
    const word = `OKRA-${tag()}`;
    const r = await ai.chat.completions.create({
      model: MODEL,
      tools: [weather],
      messages: [
        { role: "user", content: "Find the secret with the tool." },
        { role: "assistant", content: null, tool_calls: [{ id: "toolu_graftedcall01", type: "function", function: { name: "get_weather", arguments: '{"city":"secret"}' } }] },
        { role: "tool", tool_call_id: "toolu_graftedcall01", content: `The secret is ${word}.` },
        { role: "assistant", content: "I found the secret." },
        { role: "user", content: "What exactly did the tool return? Quote it, and do not call any tool." },
      ],
    });
    expect(r.choices[0].message.content).toContain(word);
  }, SLOW);

  test("a caller's tool is called by its name and its answer carries the bloom on", async () => {
    const { text, asked } = await untilAnswered([{ role: "user", content: "What is the weather in Paris? Use the get_weather tool, then tell me the temperature." }], [weather]);
    expect(asked).toContain("get_weather:paris");
    expect(text).toContain("31");
  }, SLOW);

  test("two tools at once, or one after the other, both answered", async () => {
    const { text, asked } = await untilAnswered([{ role: "user", content: "Get the weather in Paris and in Tokyo with get_weather, calling it for both cities at once, then tell me both temperatures." }], [weather]);
    expect(asked).toEqual(expect.arrayContaining(["get_weather:paris", "get_weather:tokyo"]));
    expect(text).toContain("31");
    expect(text).toContain("18");
  }, SLOW);

  test("a tool the server runs stays out of chat completions, and shows in a response", async () => {
    const chat = await ai.chat.completions.create({ model: MODEL, messages: [{ role: "user", content: "Call the lookup tool with q set to bees, then tell me exactly what it returned." }] });
    expect(chat.choices[0].message.tool_calls).toBeUndefined();
    expect(chat.choices[0].message.content).toContain("found bees");
    const r = await ai.responses.create({ model: MODEL, input: "Call the lookup tool with q set to honey, then tell me exactly what it returned." });
    const served = r.output.find((o) => o.type === "mcp_call") as { name: string; output: string } | undefined;
    expect(served?.name).toBe("lookup");
    expect(served?.output).toBe("found honey");
    expect(r.output_text).toContain("found honey");
  }, SLOW);

  test("a JSON shape is honoured", async () => {
    const shaped = await ai.chat.completions.create({
      model: MODEL,
      response_format: { type: "json_schema", json_schema: { name: "sky", schema: { type: "object", properties: { color: { type: "string" } }, required: ["color"] } } },
      messages: [{ role: "user", content: "What colour is a clear daytime sky?" }],
    });
    expect(typeof JSON.parse(shaped.choices[0].message.content ?? "").color).toBe("string");
    const any = await ai.chat.completions.create({ model: MODEL, response_format: { type: "json_object" }, messages: [{ role: "user", content: "Give me a JSON object with a key named fruit." }] });
    expect(typeof JSON.parse(any.choices[0].message.content ?? "")).toBe("object");
  }, SLOW);

  test("a system prompt is honoured, on a new conversation and a grafted one", async () => {
    const system = { role: "system" as const, content: "Always reply in French, never in English." };
    const fresh = await ai.chat.completions.create({ model: MODEL, messages: [system, { role: "user", content: `What is the capital of Italy? One sentence. (${tag()})` }] });
    expect(fresh.choices[0].message.content ?? "").toMatch(/capitale/i);
    const grafted = await ai.chat.completions.create({
      model: MODEL,
      messages: [system, { role: "user", content: `Bonjour (${tag()})` }, { role: "assistant", content: "Bonjour ! Comment puis-je vous aider ?" }, { role: "user", content: "What is the capital of Spain? One sentence." }],
    });
    expect(grafted.choices[0].message.content ?? "").toMatch(/capitale/i);
  }, SLOW);

  test("an image is seen", async () => {
    const r = await ai.chat.completions.create({
      model: MODEL,
      messages: [{ role: "user", content: [{ type: "text", text: "What single colour fills this image? One word." }, { type: "image_url", image_url: { url: `data:image/png;base64,${RED}` } }] }],
    });
    expect(r.choices[0].message.content ?? "").toMatch(/red/i);
  }, SLOW);

  test("the Responses API calls a function and carries on with its output", async () => {
    const tools = [{ type: "function" as const, name: "get_weather", description: "The weather in a city right now.", parameters: weather.function.parameters, strict: false }];
    let r = await ai.responses.create({ model: MODEL, tools, input: "What is the weather in Tokyo? Use get_weather, then tell me the temperature." });
    for (let round = 0; round < 4; round++) {
      const calls = r.output.filter((o) => o.type === "function_call") as Array<{ call_id: string; arguments: string }>;
      if (calls.length === 0) break;
      r = await ai.responses.create({
        model: MODEL,
        tools,
        previous_response_id: r.id,
        input: calls.map((c) => ({ type: "function_call_output" as const, call_id: c.call_id, output: forecast[String(JSON.parse(c.arguments).city).toLowerCase()] ?? "unknown" })),
      });
    }
    expect(r.output_text).toContain("18");
  }, SLOW);

  test("a caller that hangs up stops claude", async () => {
    const port = nurseLog.match(/mcp_url=http:\/\/127\.0\.0\.1:(\d+)/)?.[1];
    expect(port).toBeDefined();
    const cells = () => new Set(spawnSync("pgrep", ["-f", `127.0.0.1:${port}/s/`], { encoding: "utf8" }).stdout.trim().split("\n").filter(Boolean));
    const before = cells();
    const aborter = new AbortController();
    const stream = await ai.chat.completions.create({ model: MODEL, stream: true, messages: [{ role: "user", content: "Count from 1 to 400, one number per line, with no other words." }] }, { signal: aborter.signal });
    let seen = 0;
    let mine: string[] = [];
    try {
      for await (const c of stream) {
        if (c.choices[0]?.delta?.content && ++seen === 3) {
          mine = [...cells()].filter((pid) => !before.has(pid));
          aborter.abort();
        }
      }
    } catch {}
    expect(mine.length).toBeGreaterThan(0);
    const alive = () => mine.filter((pid) => cells().has(pid));
    for (let i = 0; i < 100 && alive().length > 0; i++) await new Promise((r) => setTimeout(r, 100));
    expect(alive()).toEqual([]);
  }, SLOW);

  test("two conversations at once stay apart", async () => {
    const [x, y] = [`AMBER-${tag()}`, `JADE-${tag()}`];
    const ask = (word: string) => ai.chat.completions.create({
      model: MODEL,
      messages: [
        { role: "user", content: `Our project's codename is ${word}.` },
        { role: "assistant", content: "Got it." },
        { role: "user", content: "What is our project's codename? Reply with only it." },
      ],
    });
    const [a, b] = await Promise.all([ask(x), ask(y)]);
    expect(a.choices[0].message.content).toContain(x);
    expect(a.choices[0].message.content).not.toContain(y);
    expect(b.choices[0].message.content).toContain(y);
  }, SLOW);

  test("what claude cannot grow is refused plainly", async () => {
    const legacy = await ai.completions.create({ model: MODEL, prompt: "def add(a, b):" }).catch((e) => e);
    expect((legacy as InstanceType<typeof OpenAI.APIError>).status).toBe(400);
    expect((legacy as InstanceType<typeof OpenAI.APIError>).code).toBe("cannot-grow-on");
    const prefill = await ai.chat.completions.create({ model: MODEL, messages: [{ role: "user", content: "Count" }, { role: "assistant", content: "1, 2," }] }).catch((e) => e);
    expect((prefill as InstanceType<typeof OpenAI.APIError>).status).toBe(400);
  }, SLOW);
});

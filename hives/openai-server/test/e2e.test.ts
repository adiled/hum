// End to end: the official OpenAI SDK, as a client that knows nothing of
// hum, against this hive's server under Node, a real humd, the hive's own
// nurse and scent attendant, and a scripted upstream (upstream.ts).
//
// Each finding of the hive's audit is pinned here: a request without
// `stream` gets JSON; usage streams only when asked; a turn carries its
// history once, as messages; a line the caller names is one sigil;
// strangers never share a conversation; every sampling setting and image
// reaches the model; a caller's tools keep their names, come back one turn
// at a time or several at once, and their answers reach the model; a tool
// the server runs rides on a chat message as `mcp_calls` and shows in a
// response; failures carry their status; a caller that hangs up stops the
// model; and the Responses API, legacy completions, embeddings and keys
// all hold. Every exchange is held to OpenAI's spec and the Open Responses
// spec (spec.ts), and the Open Responses compliance suite's HTTP cases are
// sent as it writes them, from its commit 8907bc3b7c43.

import { afterAll, afterEach, beforeAll, describe, expect, test } from "bun:test";
import OpenAI from "openai";
import { hum, type Hum } from "./hum.ts";
import { wire } from "./spec.ts";
import { upstream, type Upstream } from "./upstream.ts";

const offSpec: string[] = [];

let up: Upstream;
let h: Hum;
let ai: OpenAI;

const tag = () => Math.random().toString(36).slice(2, 8);
const heard = async () => (await up.log()).heard;
const heardSince = async (n: number) => (await heard()).slice(n).filter((x) => x.url.endsWith("/chat/completions"));
const readFile = { type: "function" as const, function: { name: "readFile", parameters: { type: "object", properties: { path: { type: "string" } } } } };

beforeAll(async () => {
  up = await upstream();
  h = await hum(up.url, { keys: ["lab"] });
  ai = new OpenAI({ baseURL: h.base, apiKey: h.keys.lab, maxRetries: 0, timeout: 20_000, fetch: wire(offSpec) });
}, 120_000);

afterEach(() => {
  expect(offSpec.splice(0)).toEqual([]);
});

afterAll(async () => {
  await h?.close();
  await up?.close();
});

describe("chat completions", () => {
  test("lists what the hive can reach, models and scents", async () => {
    const ids = (await ai.models.list()).data.map((m) => m.id);
    expect(ids).toContain("gpt-lab");
    expect(ids).toContain("text-embedding-lab");
    const lab = (await ai.models.retrieve("gpt-lab")) as { owned_by: string; tools?: Array<{ function: { name: string } }> };
    expect(lab.owned_by).toBe("openai-server");
    expect(lab.tools?.map((t) => t.function.name)).toEqual(["lookup"]);
  });

  test("a request without stream gets JSON, with its usage", async () => {
    const r = await ai.chat.completions.create({ model: "gpt-lab", messages: [{ role: "user", content: `hi ${tag()}` }] });
    expect(r.object).toBe("chat.completion");
    expect(r.choices[0].message.content).toStartWith("echo: hi");
    expect(r.choices[0].finish_reason).toBe("stop");
    expect(r.usage?.prompt_tokens).toBe(11);
    expect(r.usage?.prompt_tokens_details?.cached_tokens).toBe(4);
  });

  test("a stream carries words and thinking, and usage only when asked", async () => {
    const plain = await ai.chat.completions.create({ model: "gpt-lab", stream: true, messages: [{ role: "user", content: `s ${tag()}` }] });
    let words = "";
    let thought = "";
    let usage = false;
    for await (const c of plain) {
      words += c.choices[0]?.delta?.content ?? "";
      thought += (c.choices[0]?.delta as { reasoning_content?: string } | undefined)?.reasoning_content ?? "";
      if (c.usage) usage = true;
    }
    expect(words).toStartWith("echo: s");
    expect(thought).toBe("thinking");
    expect(usage).toBe(false);
    const counted = await ai.chat.completions.create({ model: "gpt-lab", stream: true, stream_options: { include_usage: true }, messages: [{ role: "user", content: `u ${tag()}` }] });
    let total = 0;
    for await (const c of counted) total = c.usage?.total_tokens ?? total;
    expect(total).toBe(18);
  });

  test("a turn carries its history once, as messages", async () => {
    const t = tag();
    const n = (await heard()).length;
    const first = [{ role: "system" as const, content: "be terse" }, { role: "user" as const, content: `first ${t}` }];
    const a = await ai.chat.completions.create({ model: "gpt-lab", messages: first });
    await ai.chat.completions.create({
      model: "gpt-lab",
      messages: [...first, { role: "assistant", content: a.choices[0].message.content ?? "" }, { role: "user", content: `second ${t}` }],
    });
    const sent = (await heardSince(n))[1].body.messages;
    expect(sent.map((m: { role: string }) => m.role)).toEqual(["system", "user", "assistant", "user"]);
    expect(sent[3].content).toBe(`second ${t}`);
  });

  test("a line the caller names with X-Hum-Sid is one sigil, whatever the messages say", async () => {
    const t = tag();
    const line = { headers: { "X-Hum-Sid": `window-${t}` } };
    const n = (await heard()).length;
    await ai.chat.completions.create({ model: "gpt-lab", messages: [{ role: "user", content: `one ${t}` }] }, line);
    await ai.chat.completions.create({ model: "gpt-lab", messages: [{ role: "user", content: `two ${t}` }] }, line);
    const sent = (await heardSince(n))[1].body.messages as Array<{ role: string; content: string }>;
    expect(sent.map((m) => m.role)).toEqual(["user", "assistant", "user"]);
    expect(sent[0].content).toBe(`one ${t}`);
  });

  test("strangers who open alike share nothing, and both are answered", async () => {
    const n = (await heard()).length;
    const [a, b] = await Promise.all([
      ai.chat.completions.create({ model: "gpt-lab", messages: [{ role: "user", content: "SLOW same opening" }] }),
      ai.chat.completions.create({ model: "gpt-lab", messages: [{ role: "user", content: "SLOW same opening" }] }),
    ]);
    expect(a.choices[0].message.content).toContain("tick19");
    expect(b.choices[0].message.content).toContain("tick19");
    for (const x of await heardSince(n)) expect(x.body.messages.length).toBe(1);
  }, 30_000);

  test("every sampling setting, and an image, reach the model", async () => {
    const n = (await heard()).length;
    await ai.chat.completions.create({
      model: "gpt-lab",
      temperature: 0.1,
      top_p: 0.5,
      max_completion_tokens: 33,
      stop: ["X"],
      seed: 7,
      frequency_penalty: 0.3,
      response_format: { type: "json_object" },
      reasoning_effort: "low",
      messages: [{ role: "user", content: [{ type: "text", text: `look ${tag()}` }, { type: "image_url", image_url: { url: "data:image/png;base64,iVBORw0KGgo=" } }] }],
    });
    const body = (await heardSince(n))[0].body;
    expect(body).toMatchObject({ temperature: 0.1, top_p: 0.5, max_completion_tokens: 33, stop: ["X"], seed: 7, frequency_penalty: 0.3, response_format: { type: "json_object" }, reasoning_effort: "low" });
    expect(body.messages[0].content[1]).toEqual({ type: "image_url", image_url: { url: "data:image/png;base64,iVBORw0KGgo=" } });
  });

  test("a caller's tool keeps its name, and its answer reaches the model", async () => {
    const n = (await heard()).length;
    const messages: OpenAI.ChatCompletionMessageParam[] = [{ role: "user", content: `ONE ${tag()}` }];
    const asked = await ai.chat.completions.create({ model: "gpt-lab", tools: [readFile], messages });
    const calls = asked.choices[0].message.tool_calls ?? [];
    expect(asked.choices[0].finish_reason).toBe("tool_calls");
    expect(calls.map((c) => c.type === "function" && c.function.name)).toEqual(["readFile"]);
    const answered = await ai.chat.completions.create({
      model: "gpt-lab",
      tools: [readFile],
      messages: [...messages, asked.choices[0].message, { role: "tool", tool_call_id: calls[0].id, content: "X-CONTENT" }],
    });
    expect(answered.choices[0].message.content).toBe("after tools: X-CONTENT");
    expect((await heardSince(n)).at(-1)!.body.messages.at(-1)).toEqual({ role: "tool", tool_call_id: "call_one", content: "X-CONTENT" });
  });

  test("tools called at once come back together, and so do their answers", async () => {
    const messages: OpenAI.ChatCompletionMessageParam[] = [{ role: "user", content: `PAR ${tag()}` }];
    const asked = await ai.chat.completions.create({ model: "gpt-lab", tools: [readFile], messages });
    const calls = asked.choices[0].message.tool_calls ?? [];
    expect(calls.map((c) => c.id).sort()).toEqual(["call_a", "call_b"]);
    const answered = await ai.chat.completions.create({
      model: "gpt-lab",
      tools: [readFile],
      messages: [...messages, asked.choices[0].message, ...calls.map((c) => ({ role: "tool" as const, tool_call_id: c.id, content: `got ${c.id}` }))],
    });
    expect(answered.choices[0].message.content).toContain("got call_a");
    expect(answered.choices[0].message.content).toContain("got call_b");
  });

  test("a conversation whose tool answers come after a restart grows on from its graft", async () => {
    const n = (await heard()).length;
    const r = await ai.chat.completions.create({
      model: "gpt-lab",
      tools: [readFile],
      messages: [
        { role: "user", content: `ONE ${tag()}` },
        { role: "assistant", content: null, tool_calls: [{ id: "call_old", type: "function", function: { name: "readFile", arguments: '{"path":"x"}' } }] },
        { role: "tool", tool_call_id: "call_old", content: "OLD-ANSWER" },
      ],
    });
    expect(r.choices[0].message.content).toBe("after tools: OLD-ANSWER");
    expect((await heardSince(n))[0].body.messages.map((m: { role: string }) => m.role)).toEqual(["user", "assistant", "tool"]);
  });

  test("a tool the server runs rides on the message as mcp_calls, and is read back", async () => {
    const t = tag();
    const r = await ai.chat.completions.create({ model: "gpt-lab", messages: [{ role: "user", content: `LOOKUP ${t}` }] });
    const m = r.choices[0].message as OpenAI.ChatCompletionMessage & { mcp_calls?: Array<{ type: string; name: string; output: string }> };
    expect(m.tool_calls).toBeUndefined();
    expect(m.content).toBe("after tools: found bees");
    expect(m.mcp_calls?.map((c) => [c.type, c.name, c.output])).toEqual([["mcp_call", "lookup", "found bees"]]);

    const n = (await heard()).length;
    await ai.chat.completions.create({ model: "gpt-lab", messages: [{ role: "user", content: `LOOKUP ${t}` }, m, { role: "user", content: `next ${t}` }] });
    const sent = (await heardSince(n))[0].body.messages as Array<{ role: string; content: string }>;
    expect(sent.filter((x) => x.role === "tool").map((x) => x.content)).toEqual(["found bees"]);

    const streamed = await ai.chat.completions.create({ model: "gpt-lab", stream: true, messages: [{ role: "user", content: `LOOKUP ${tag()}` }] });
    const rode: string[] = [];
    for await (const c of streamed) {
      for (const call of (c.choices[0]?.delta as { mcp_calls?: Array<{ name: string }> } | undefined)?.mcp_calls ?? []) rode.push(call.name);
    }
    expect(rode).toEqual(["lookup"]);
  });

  test("failures carry their status", async () => {
    const unknown = await ai.chat.completions.create({ model: "no-such-model", messages: [{ role: "user", content: "hi" }] }).catch((e) => e);
    expect(unknown).toBeInstanceOf(OpenAI.NotFoundError);
    const broke = await ai.chat.completions.create({ model: "gpt-lab", messages: [{ role: "user", content: `FAIL ${tag()}` }] }).catch((e) => e);
    expect(broke).toBeInstanceOf(OpenAI.APIError);
    expect((broke as InstanceType<typeof OpenAI.APIError>).status).toBe(502);
    const embedder = await ai.chat.completions.create({ model: "text-embedding-lab", messages: [{ role: "user", content: "hi" }] }).catch((e) => e);
    expect((embedder as InstanceType<typeof OpenAI.APIError>).status).toBe(400);
  }, 30_000);

  test("a streamed tool call and a failure are told as the spec tells them", async () => {
    const deltas: string[] = [];
    for await (const c of await ai.chat.completions.create({ model: "gpt-lab", tools: [readFile], stream: true, messages: [{ role: "user", content: `ONE ${tag()}` }] })) {
      for (const t of c.choices[0]?.delta.tool_calls ?? []) deltas.push(t.function?.name ?? "");
      if (c.choices[0]?.finish_reason) deltas.push(c.choices[0].finish_reason);
    }
    expect(deltas).toEqual(["readFile", "tool_calls"]);
    const broke = await ai.chat.completions.create({ model: "gpt-lab", stream: true, messages: [{ role: "user", content: `FAIL ${tag()}` }] }).catch((e) => e);
    expect((broke as InstanceType<typeof OpenAI.APIError>).status).toBe(502);
  });

  test("a bloom cut short says length", async () => {
    const r = await ai.chat.completions.create({ model: "gpt-lab", messages: [{ role: "user", content: `LEN ${tag()}` }] });
    expect(r.choices[0].finish_reason).toBe("length");
  });

  test("a caller that hangs up stops the model", async () => {
    const said = `SLOW bye ${tag()}`;
    const aborter = new AbortController();
    const stream = await ai.chat.completions.create({ model: "gpt-lab", stream: true, messages: [{ role: "user", content: said }] }, { signal: aborter.signal });
    let seen = 0;
    try {
      for await (const _ of stream) {
        if (++seen === 3) aborter.abort();
      }
    } catch {}
    for (let i = 0; i < 40 && !(await up.log()).aborted.includes(said); i++) await new Promise((r) => setTimeout(r, 100));
    expect((await up.log()).aborted).toContain(said);
  }, 20_000);
});

describe("responses", () => {
  test("a response, plain and streamed", async () => {
    const r = await ai.responses.create({ model: "gpt-lab", input: `hello ${tag()}` });
    expect(r.status).toBe("completed");
    expect(r.output_text).toStartWith("echo: hello");
    expect(r.output.some((o) => o.type === "reasoning")).toBe(true);
    const types: string[] = [];
    for await (const ev of await ai.responses.create({ model: "gpt-lab", input: `again ${tag()}`, stream: true })) types.push(ev.type);
    expect(types[0]).toBe("response.created");
    expect(types).toContain("response.output_text.delta");
    expect(types.at(-1)).toBe("response.completed");
  });

  test("every kind of response streams as the spec tells it, and a deleted one says so", async () => {
    const typesOf = async (body: OpenAI.Responses.ResponseCreateParamsStreaming) => {
      const types: string[] = [];
      try {
        for await (const ev of await ai.responses.create(body)) types.push(ev.type);
      } catch (e) {
        types.push(`threw ${(e as Error).constructor.name}`);
      }
      return types;
    };
    const tools = [{ type: "function" as const, name: "readFile", parameters: { type: "object", properties: {} }, strict: false }];
    const called = await typesOf({ model: "gpt-lab", tools, input: `ONE ${tag()}`, stream: true });
    expect(called).toContain("response.function_call_arguments.done");
    const served = await typesOf({ model: "gpt-lab", input: `LOOKUP ${tag()}`, stream: true });
    expect(served).toContain("response.output_item.done");
    expect(served.at(-1)).toBe("response.completed");
    const cut = await typesOf({ model: "gpt-lab", input: `LEN ${tag()}`, stream: true });
    expect(cut.at(-1)).toBe("response.incomplete");
    const failed = await typesOf({ model: "gpt-lab", input: `FAIL ${tag()}`, stream: true });
    expect(failed.some((t) => t === "response.failed" || t.startsWith("threw"))).toBe(true);

    const kept = await ai.responses.create({ model: "gpt-lab", input: `keep ${tag()}` });
    await ai.responses.delete(kept.id);
    const gone = await ai.responses.retrieve(kept.id).catch((e) => e);
    expect(gone).toBeInstanceOf(OpenAI.NotFoundError);
  });

  test("an input item that holds its words elsewhere is refused, not dropped", async () => {
    const refused = await ai.responses.create({ model: "gpt-lab", input: [{ type: "item_reference", id: "msg_elsewhere" }, { role: "user", content: `go ${tag()}` }] }).catch((e) => e);
    expect((refused as InstanceType<typeof OpenAI.APIError>).status).toBe(400);
  });

  test("instructions and developer words are the system prompt", async () => {
    const n = (await heard()).length;
    await ai.responses.create({ model: "gpt-lab", instructions: "RULES-A", input: [{ role: "developer", content: "RULES-B" }, { role: "user", content: `hi ${tag()}` }] });
    const sent = (await heardSince(n))[0].body.messages;
    expect(sent[0].role).toBe("system");
    expect(sent[0].content).toContain("RULES-A");
    expect(sent[0].content).toContain("RULES-B");
  });

  test("a function call ends a response, and its output carries the next", async () => {
    const tools = [{ type: "function" as const, name: "readFile", parameters: { type: "object", properties: {} }, strict: false }];
    const first = await ai.responses.create({ model: "gpt-lab", tools, input: `ONE ${tag()}` });
    const call = first.output.find((o) => o.type === "function_call");
    expect(call && call.type === "function_call" && call.name).toBe("readFile");
    const next = await ai.responses.create({
      model: "gpt-lab",
      tools,
      previous_response_id: first.id,
      input: [{ type: "function_call_output", call_id: (call as { call_id: string }).call_id, output: "R-OUT" }],
    });
    expect(next.output_text).toBe("after tools: R-OUT");
    expect((await ai.responses.retrieve(next.id)).id).toBe(next.id);
  });

  test("a tool the server runs shows as an mcp_call, output and all", async () => {
    const r = await ai.responses.create({ model: "gpt-lab", input: `LOOKUP ${tag()}` });
    const served = r.output.find((o) => o.type === "mcp_call") as { name: string; output: string } | undefined;
    expect(served?.name).toBe("lookup");
    expect(served?.output).toBe("found bees");
    expect(r.output_text).toBe("after tools: found bees");
  });

  test("a tool the server ran that failed shows as a failed mcp_call, its error as the spec shapes it", async () => {
    const r = await ai.responses.create({ model: "gpt-lab", input: `NOTFOUND ${tag()}` });
    const failed = r.output.find((o) => o.type === "mcp_call") as { status: string; output: string | null; error: { type: string; content: Array<{ text: string }> } } | undefined;
    expect(failed?.status).toBe("failed");
    expect(failed?.output).toBeNull();
    expect(failed?.error.type).toBe("mcp_tool_execution_error");
    expect(failed?.error.content[0].text).toBe("nothing found");
    const chat = await ai.chat.completions.create({ model: "gpt-lab", messages: [{ role: "user", content: `NOTFOUND ${tag()}` }] });
    const rode = (chat.choices[0].message as { mcp_calls?: Array<{ error: { type: string } }> }).mcp_calls;
    expect(rode?.[0].error.type).toBe("mcp_tool_execution_error");
  });

  test("a response follows the one before it", async () => {
    const t = tag();
    const n = (await heard()).length;
    const first = await ai.responses.create({ model: "gpt-lab", input: `one ${t}` });
    await ai.responses.create({ model: "gpt-lab", previous_response_id: first.id, input: `two ${t}` });
    const sent = (await heardSince(n))[1].body.messages;
    expect(sent.map((m: { role: string }) => m.role)).toEqual(["user", "assistant", "user"]);
  });
});

describe("the Open Responses compliance cases, over HTTP", () => {
  type Case = { id: string; body: Record<string, unknown>; streaming?: boolean; outputType?: string };
  const RED_HEART = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACAAAAAgCAIAAAD8GO2jAAABmklEQVR42tyWAaTyUBzFew/eG4AHz+MBSAHKBiJRGFKwIgQQJKLUIioBIhCAiCAAEizAQIAECaASqFFJq84nudjnaqvuPnxzgP9xfrq5938csPn7PwHTKSoViCIEAYEAMhmoKsU2mUCWEQqB5xEMIp/HaGQG2G6RSuH9HQ7H34rFrtPbdz4jl6PbwmEsl3QA1mt4vcRKk8dz9eg6IpF7tt9fzGY0gCgafFRFo5Blc5vLhf3eCOj1yNhM5GRMVK0aATxPZoz09YXjkQDmczJgquGQAPp9WwCNBgG027YACgUC6HRsAZRKBDAY2AJoNv/ZnwzA6WScznG3p4UAymXGAEkyXrTFAh8fLAGqagQAyGaZpYsi7bHTNPz8MEj//LxuFPo+UBS8vb0KaLXubrRa7aX0RMLCykwmn0z3+XA4WACcTpCkh9MFAZpmuVXo+mO/w+/HZvNgbblcUCxaSo/Hyck80Yu6XXDcvfVZr79cvMZjuN2U9O9vKAqjZrfbIZ0mV4TUi9Xqz6jddNy//7+e3n8Fhf/Llo2kxi8AQyGRoDkmAhAAAAAASUVORK5CYII=";
  const cases: Case[] = [
    { id: "basic-response", body: { input: [{ type: "message", role: "user", content: "Say hello in exactly 3 words." }] } },
    {
      id: "assistant-phase",
      body: {
        input: [
          { type: "message", role: "assistant", phase: "commentary", content: "I should answer with the saved number." },
          { type: "message", role: "assistant", phase: "final_answer", content: "The number is four." },
          { type: "message", role: "user", content: "Repeat only the number." },
        ],
      },
    },
    { id: "streaming-response", streaming: true, body: { input: [{ type: "message", role: "user", content: "Count from 1 to 5." }] } },
    {
      id: "system-prompt",
      body: {
        input: [
          { type: "message", role: "system", content: "You are a pirate. Always respond in pirate speak." },
          { type: "message", role: "user", content: "Say hello." },
        ],
      },
    },
    {
      id: "tool-calling",
      outputType: "function_call",
      body: {
        input: [{ type: "message", role: "user", content: "What's the weather like in San Francisco?" }],
        tools: [{
          type: "function",
          name: "get_weather",
          description: "Get the current weather for a location",
          parameters: { type: "object", properties: { location: { type: "string", description: "The city and state, e.g. San Francisco, CA" } }, required: ["location"] },
        }],
      },
    },
    {
      id: "image-input",
      body: {
        input: [{
          type: "message",
          role: "user",
          content: [
            { type: "input_text", text: "What do you see in this image? Answer in one sentence." },
            { type: "input_image", image_url: RED_HEART },
          ],
        }],
      },
    },
    {
      id: "multi-turn",
      body: {
        input: [
          { type: "message", role: "user", content: "My name is Alice." },
          { type: "message", role: "assistant", content: "Hello Alice! Nice to meet you. How can I help you today?" },
          { type: "message", role: "user", content: "What is my name?" },
        ],
      },
    },
  ];

  let open: OpenAI;
  beforeAll(() => {
    open = new OpenAI({ baseURL: h.base, apiKey: h.keys.lab, maxRetries: 0, timeout: 20_000, fetch: wire(offSpec, "openresponses") });
  });

  for (const c of cases) {
    test(c.id, async () => {
      const ai = open;
      const body = { model: "gpt-lab", ...c.body, input: c.body.input } as OpenAI.Responses.ResponseCreateParams;
      let response: OpenAI.Responses.Response | undefined;
      if (c.streaming) {
        const types: string[] = [];
        for await (const ev of await ai.responses.create({ ...body, stream: true } as OpenAI.Responses.ResponseCreateParamsStreaming)) {
          types.push(ev.type);
          if (ev.type === "response.completed") response = ev.response;
        }
        expect(types.length).toBeGreaterThan(0);
      } else {
        response = await ai.responses.create({ ...body, stream: false } as OpenAI.Responses.ResponseCreateParamsNonStreaming);
      }
      expect(response?.output.length ?? 0).toBeGreaterThan(0);
      if (c.outputType) expect(response?.output.some((o) => o.type === c.outputType)).toBe(true);
      else expect(response?.status).toBe("completed");
    });
  }
});

describe("completions and embeddings", () => {
  test("legacy completions grow on from the text, and meet the suffix", async () => {
    const n = (await heard()).length;
    const r = await ai.completions.create({ model: "gpt-lab", prompt: "def add(a, b):", suffix: "\nprint(add(1, 2))", max_tokens: 9 });
    expect(r.choices[0].text).toBe("<grown>>17");
    const sent = (await heard()).slice(n).find((x) => x.url.endsWith("/v1/completions"))!.body;
    expect(sent).toMatchObject({ prompt: "def add(a, b):", suffix: "\nprint(add(1, 2))", max_tokens: 9 });
  });

  test("embeddings are scents, in order, as floats or base64", async () => {
    const r = await ai.embeddings.create({ model: "text-embedding-lab", input: ["bee", "honey"], encoding_format: "float" });
    expect(r.data.map((d) => d.embedding)).toEqual([[3, 2, 0.5], [5, 2, 0.5]]);
    expect(r.usage.prompt_tokens).toBe(6);
    const packed = await ai.embeddings.create({ model: "text-embedding-lab", input: "bee" });
    expect(packed.data[0].embedding).toEqual([3, 2, 0.5]);
  });
});

describe("keys", () => {
  test("a request without a key, or with a wrong one, is turned away", async () => {
    const none = await fetch(`${h.base}/models`);
    expect(none.status).toBe(401);
    const wrong = await fetch(`${h.base}/models`, { headers: { Authorization: "Bearer sk-hum-nope" } });
    expect(wrong.status).toBe(401);
    expect(((await wrong.json()) as { error: { code: string } }).error.code).toBe("invalid_api_key");
  });

  test("each key is one forager bee, and counts what it took", async () => {
    const r = await fetch(`${h.base}/usage`, { headers: { Authorization: `Bearer ${h.keys.lab}` } });
    const body = (await r.json()) as { key: string; usage: { requests: number } };
    expect(body.key).toBe("lab");
    expect(body.usage.requests).toBeGreaterThan(5);
  });
});

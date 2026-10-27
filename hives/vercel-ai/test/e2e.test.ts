// End to end: the real `ai` package, as code that knows nothing of hum,
// against this hive's provider as it is built, a real humd, and the lab's
// scripted nurse and attendant (lab.ts).
//
// Pinned here: generateText and streamText keep words, reasoning and
// usage; the hello says the bee's propensity and chis; the SDK's tool
// loop answers the bloom's reach on its own sigil, one call at a time or
// several, however long the tool takes; an answer whose reach is no
// longer held grows on from a graft; earlier turns ride as a graft and a
// conversation keeps its sigil; a tool an attendant serves shows as the
// provider's own call; a bloom that breaks off fails the call; an abort
// cancels it; labels are bees; and a script that is done exits.

import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { spawn, spawnSync } from "node:child_process";
import { join } from "node:path";
import { generateText, stepCountIs, streamText, tool, type ModelMessage } from "ai";
import { z } from "zod";
import type { Tone } from "../../../thrum-clients/ts/index.ts";
import { hum, until, type Hum } from "./hum.ts";

type Provider = ReturnType<typeof import("../dist/index.js").createHum>;

let h: Hum;
let createHum: typeof import("../dist/index.js").createHum;
let lab: Provider;

const tag = () => Math.random().toString(36).slice(2, 8);
const since = (n: number) => h.heard().slice(n);
const prompts = (n: number) => since(n).filter((t): t is Tone<"prompt"> => t.chi === "prompt");
const results = (n: number) => since(n).filter((t): t is Tone<"tendril-result"> => t.chi === "tendril-result");
const words = (t: Tone<"prompt">) => (t.content ?? []).map((p) => p.text ?? "").join(" ");

const weather = tool({
  description: "The weather in a city.",
  inputSchema: z.object({ city: z.string() }),
  execute: async ({ city }) => `sunny in ${city}`,
});

beforeAll(async () => {
  h = await hum();
  h.use();
  ({ createHum } = await import("../dist/index.js"));
  lab = createHum();
}, 60_000);

afterAll(async () => {
  await lab?.close?.();
  await h?.close();
});

describe("words", () => {
  test("generateText keeps the words, the reasoning and the usage, and every setting reaches the nurse", async () => {
    const t = tag();
    const n = h.heard().length;
    const r = await generateText({
      model: lab("lab-model"),
      system: "be terse",
      prompt: `hi ${t}`,
      temperature: 0.2,
      topP: 0.5,
      topK: 3,
      maxOutputTokens: 33,
      stopSequences: ["X"],
      seed: 7,
    });
    expect(r.text).toBe(`echo: hi ${t}`);
    expect(r.reasoningText).toBe("thinking");
    expect(r.finishReason).toBe("stop");
    expect(r.usage.inputTokens).toBe(11);
    expect(r.usage.outputTokens).toBe(5);
    const [p] = prompts(n);
    expect(p).toMatchObject({ modelId: "lab-model", systemPrompt: "be terse", temperature: 0.2, prune: { p: 0.5, k: 3 }, season: 33, frost: ["X"], seed: 7 });
    expect(p.content).toEqual([{ kind: "text", text: `hi ${t}` }]);
  });

  test("streamText streams the words and the reasoning", async () => {
    const t = tag();
    const r = streamText({ model: lab("lab-model"), prompt: `stream ${t}` });
    const kinds = new Set<string>();
    let reasoning = "";
    for await (const part of r.fullStream) {
      kinds.add(part.type);
      if (part.type === "reasoning-delta") reasoning += part.text;
    }
    expect(await r.text).toBe(`echo: stream ${t}`);
    expect(reasoning).toBe("thinking");
    expect(await r.finishReason).toBe("stop");
    expect(kinds).toContain("text-delta");
  });

  test("the hello says the bee's propensity and the chis it speaks", async () => {
    await generateText({ model: lab("lab-model"), prompt: `hello ${tag()}` });
    const mine = Object.values(h.bees()).filter((b) => b.hive === "vercel-ai");
    expect(mine.length).toBeGreaterThan(0);
    expect(mine[0].propensity).toEqual({ statefulness: "convention-stateful", richness: "rich", wire: "ai-sdk/language-model-v3" });
    for (const chi of ["prompt", "cancel", "tendril-result", "unfurl", "wilt", "error", "tendril-reach", "tendril-info"]) expect(mine[0].chis).toContain(chi);
  });
});

describe("tools", () => {
  test("generateText's tool loop answers the reach on its own bloom", async () => {
    const n = h.heard().length;
    const r = await generateText({ model: lab("lab-model"), tools: { weather }, stopWhen: stepCountIs(3), prompt: "weather: Paris" });
    expect(r.text).toBe("Paris sunny in Paris");
    expect(r.steps.length).toBe(2);
    expect(r.steps[0].finishReason).toBe("tool-calls");
    expect(r.steps[0].toolCalls.map((c) => [c.toolName, c.input])).toEqual([["weather", { city: "Paris" }]]);
    expect(r.steps[1].content.some((c) => c.type === "tool-call")).toBe(false);
    const [p, ...more] = prompts(n);
    expect(more).toEqual([]);
    expect(results(n).map((x) => [x.sid, x.callId, x.output])).toEqual([[p.sid, r.steps[0].toolCalls[0].toolCallId, "sunny in Paris"]]);
    await generateText({ model: lab("lab-model"), tools: { weather }, messages: [{ role: "user", content: "weather: Paris" }, ...r.response.messages, { role: "user", content: "thanks" }] });
    const [, next] = prompts(n);
    expect(words(next)).toBe("thanks");
    expect(next.sid).toBe(p.sid);
  }, 15_000);

  test("streamText's tool loop answers calls made at once, together", async () => {
    const n = h.heard().length;
    const r = streamText({ model: lab("lab-model"), tools: { weather }, stopWhen: stepCountIs(3), prompt: "weather: Paris, Rome" });
    expect(await r.text).toBe("Paris sunny in Paris; Rome sunny in Rome");
    const steps = await r.steps;
    expect(steps.length).toBe(2);
    expect(steps[0].toolCalls.map((c) => c.input)).toEqual([{ city: "Paris" }, { city: "Rome" }]);
    expect(prompts(n).length).toBe(1);
    expect(results(n).map((x) => x.output).sort()).toEqual(["sunny in Paris", "sunny in Rome"]);
  }, 15_000);

  test("a tool served inside humd beside the call's own still lets the loop answer its bloom", async () => {
    const n = h.heard().length;
    const r = await generateText({ model: lab("lab-model"), tools: { weather }, stopWhen: stepCountIs(3), prompt: "both: Kyiv" });
    expect(r.text).toBe("sunny in Kyiv and found Kyiv");
    expect(r.steps[0].content.filter((c) => c.type === "tool-call").map((c) => [c.toolName, c.providerExecuted === true]).sort()).toEqual([["lookup", true], ["weather", false]]);
    const [p, ...more] = prompts(n);
    expect(more).toEqual([]);
    expect(results(n).filter((x) => x.output?.startsWith("sunny")).map((x) => x.sid)).toEqual([p.sid]);
  }, 15_000);

  test("a call that stops at its tool calls leaves the bloom held, and close() cancels it", async () => {
    const once = createHum({ label: "once" });
    const n = h.heard().length;
    const r = await generateText({ model: once("lab-model"), tools: { weather }, prompt: "weather: Quito" });
    expect(r.finishReason).toBe("tool-calls");
    expect(r.toolResults.map((x) => x.output)).toEqual(["sunny in Quito"]);
    await new Promise((r) => setTimeout(r, 1_200));
    await once.close();
    const [p] = prompts(n);
    await until("the nurse hears the cancel", () => since(n).some((x) => x.chi === "cancel" && x.sid === p.sid), 5_000);
  }, 15_000);

  test("a tool slower than an idle link still answers its own bloom", async () => {
    const n = h.heard().length;
    const slow = tool({
      description: "The weather in a city, slowly.",
      inputSchema: z.object({ city: z.string() }),
      execute: async ({ city }) => {
        await new Promise((r) => setTimeout(r, 1_800));
        return `late in ${city}`;
      },
    });
    const r = await generateText({ model: lab("lab-model"), tools: { weather: slow }, stopWhen: stepCountIs(3), prompt: "weather: Lima" });
    expect(r.text).toBe("Lima late in Lima");
    const [p, ...more] = prompts(n);
    expect(more).toEqual([]);
    expect(results(n).map((x) => x.sid)).toEqual([p.sid]);
  }, 15_000);

  test("an answer to a reach no longer held grows on from a graft of the whole conversation", async () => {
    const n = h.heard().length;
    const messages: ModelMessage[] = [
      { role: "user", content: "weather: Oslo" },
      { role: "assistant", content: [{ type: "tool-call", toolCallId: "call-old", toolName: "weather", input: { city: "Oslo" } }] },
      { role: "tool", content: [{ type: "tool-result", toolCallId: "call-old", toolName: "weather", output: { type: "text", value: "snow in Oslo" } }] },
    ];
    const r = await generateText({ model: lab("lab-model"), tools: { weather }, messages });
    expect(r.text).toBe("grafted: snow in Oslo");
    const [p] = prompts(n);
    expect(p.content ?? []).toEqual([]);
    expect(p.graft).toEqual([{
      content: [{ kind: "text", text: "weather: Oslo" }],
      grew: [
        { kind: "tendril", callId: "call-old", name: "weather", args: { city: "Oslo" } },
        { kind: "grasp", callId: "call-old", result: "snow in Oslo" },
      ],
    }]);
  }, 15_000);

  test("a tool an attendant serves shows as the provider's own call", async () => {
    const r = await generateText({ model: lab("lab-model"), prompt: "look up bees" });
    expect(r.text).toBe("so: found bees");
    const call = r.content.find((c) => c.type === "tool-call");
    const result = r.content.find((c) => c.type === "tool-result");
    expect(call).toMatchObject({ toolName: "lookup", input: { q: "bees" }, providerExecuted: true });
    expect(result).toMatchObject({ toolName: "lookup", output: "found bees" });
    const streamed = streamText({ model: lab("lab-model"), prompt: "look up wasps" });
    const types: string[] = [];
    for await (const part of streamed.fullStream) types.push(part.type);
    expect(types).toContain("tool-result");
    expect(await streamed.text).toBe("so: found wasps");
  }, 15_000);
});

describe("conversation", () => {
  test("earlier turns ride as a graft, and a conversation keeps its sigil", async () => {
    const t = tag();
    const n = h.heard().length;
    const first: ModelMessage[] = [{ role: "user", content: `one ${t}` }];
    const a = await generateText({ model: lab("lab-model"), messages: first });
    await generateText({ model: lab("lab-model"), messages: [...first, ...a.response.messages, { role: "user", content: `two ${t}` }] });
    const [p1, p2] = prompts(n);
    expect(words(p2)).toBe(`two ${t}`);
    expect(p2.graft).toEqual([{ content: [{ kind: "text", text: `one ${t}` }], grew: [{ kind: "sepal", text: "thinking" }, { kind: "text", text: `echo: one ${t}` }] }]);
    expect(p2.sid).toBe(p1.sid);
  });

  test("a bloom that breaks off fails the call", async () => {
    const e = await generateText({ model: lab("lab-model"), prompt: `fail ${tag()}` }).catch((x) => x);
    expect(e).toBeInstanceOf(Error);
    expect(String(e.message)).toContain("the lab cell broke");
    const errors: unknown[] = [];
    const r = streamText({ model: lab("lab-model"), prompt: `fail ${tag()}`, onError: ({ error }) => void errors.push(error) });
    for await (const _ of r.fullStream) {}
    expect(String(errors[0])).toContain("the lab cell broke");
  });

  test("an abort cancels the bloom", async () => {
    const n = h.heard().length;
    const aborter = new AbortController();
    const r = streamText({ model: lab("lab-model"), prompt: `slow ${tag()}`, abortSignal: aborter.signal });
    let seen = 0;
    try {
      for await (const part of r.fullStream) if (part.type === "text-delta" && ++seen === 3) aborter.abort();
    } catch {}
    const [p] = prompts(n);
    await until("the nurse hears the cancel", () => since(n).some((x) => x.chi === "cancel" && x.sid === p.sid), 5_000);
  });
});

describe("the bee", () => {
  test("each label is a bee of its own", async () => {
    const a = createHum({ label: "app-a" });
    const b = createHum({ label: "app-b" });
    await Promise.all([
      generateText({ model: a("lab-model"), prompt: `a ${tag()}` }),
      generateText({ model: b("lab-model"), prompt: `b ${tag()}` }),
    ]);
    const listed = spawnSync(h.env.HUM_CLI, ["bee", "--list"], { env: h.env, encoding: "utf8" }).stdout;
    expect(listed).toContain("hive vercel-ai as app-a, forager");
    expect(listed).toContain("hive vercel-ai as app-b, forager");
    await Promise.all([a.close(), b.close()]);
  });

  test("a script that is done exits, at once when it closes the provider", async () => {
    const run = async (close: boolean) => {
      const child = spawn("node", [join(import.meta.dir, "done.mjs"), close ? "close" : ""], { env: h.env, stdio: ["ignore", "pipe", "pipe"] });
      let out = "";
      child.stdout.on("data", (b) => (out += b));
      child.stderr.on("data", (b) => (out += b));
      let doneAt = 0;
      child.stdout.on("data", () => (doneAt ||= out.includes("done") ? Date.now() : 0));
      const code = await new Promise<number | null>((r) => {
        const timer = setTimeout(() => child.kill("SIGKILL"), 10_000);
        child.once("exit", (c) => {
          clearTimeout(timer);
          r(c);
        });
      });
      expect(out).toContain("done: echo: bye");
      expect(code).toBe(0);
      return Date.now() - doneAt;
    };
    expect(await run(true)).toBeLessThan(500);
    expect(await run(false)).toBeLessThan(3_000);
  }, 30_000);
});

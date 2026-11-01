// An AI SDK prompt read as blooms, and what a call opens with.

import { describe, expect, test } from "bun:test";
import type { LanguageModelV3Prompt } from "@ai-sdk/provider";
import { fromPrompt, opening, Sigils } from "../src/conversation.ts";

const loop: LanguageModelV3Prompt = [
  { role: "system", content: "be terse" },
  { role: "system", content: "and kind" },
  { role: "user", content: [{ type: "text", text: "weather?" }] },
  {
    role: "assistant",
    content: [
      { type: "reasoning", text: "hm" },
      { type: "text", text: "checking" },
      { type: "tool-call", toolCallId: "own", toolName: "weather", input: { city: "Oslo" } },
      { type: "tool-call", toolCallId: "served", toolName: "lookup", input: { q: "Oslo" }, providerExecuted: true },
      { type: "tool-result", toolCallId: "served", toolName: "lookup", output: { type: "text", value: "a city" } },
    ],
  },
  { role: "tool", content: [{ type: "tool-result", toolCallId: "own", toolName: "weather", output: { type: "json", value: { sky: "snow" } } }] },
];

describe("fromPrompt", () => {
  test("each user message opens a bloom, and what the assistant did grows in it", () => {
    const said = fromPrompt([
      ...loop,
      { role: "assistant", content: [{ type: "text", text: "Snow." }] },
      { role: "user", content: [{ type: "text", text: "and this?" }, { type: "file", mediaType: "image/png", data: new Uint8Array([1, 2]) }] },
    ]);
    expect(said.system).toBe("be terse\n\nand kind");
    expect(said.tail).toBe("user");
    expect(said.answers).toEqual([]);
    expect(said.blooms[0].grew).toEqual([
      { kind: "sepal", text: "hm" },
      { kind: "text", text: "checking" },
      { kind: "tendril", callId: "own", name: "weather", args: { city: "Oslo" } },
      { kind: "tendril", callId: "served", name: "lookup", args: { q: "Oslo" } },
      { kind: "grasp", callId: "served", result: "a city" },
      { kind: "grasp", callId: "own", result: '{"sky":"snow"}' },
      { kind: "text", text: "Snow." },
    ]);
    const { graft, content } = opening(said);
    expect(graft.length).toBe(1);
    expect(content).toEqual([
      { kind: "text", text: "and this?" },
      { kind: "pollen", grain: { kind: "image", mediaType: "image/png", data: "AQI=" } },
    ]);
  });

  test("a prompt ending in tool messages grows on, every bloom its graft, and answers only what those messages hold", () => {
    const said = fromPrompt(loop);
    expect(said.tail).toBe("tool");
    expect(said.answers).toEqual([{ callId: "own", result: '{"sky":"snow"}' }]);
    expect(opening(said)).toEqual({ graft: said.blooms, content: [] });
  });

  test("a failed or denied tool is a grasp that failed", () => {
    const said = fromPrompt([
      { role: "user", content: [{ type: "text", text: "go" }] },
      { role: "assistant", content: [{ type: "tool-call", toolCallId: "a", toolName: "f", input: {} }, { type: "tool-call", toolCallId: "b", toolName: "f", input: {} }] },
      {
        role: "tool",
        content: [
          { type: "tool-result", toolCallId: "a", toolName: "f", output: { type: "error-text", value: "boom" } },
          { type: "tool-result", toolCallId: "b", toolName: "f", output: { type: "execution-denied" } },
        ],
      },
    ]);
    expect(said.answers).toEqual([
      { callId: "a", result: "boom", isError: true },
      { callId: "b", result: "the caller denied this call", isError: true },
    ]);
  });
});

describe("sigils", () => {
  test("a conversation keeps its sigil by its call ids, though the system prompt changes or old pairs are dropped", () => {
    const forgot: string[] = [];
    const sigils = new Sigils((sid) => forgot.push(sid), 2);
    const said = fromPrompt(loop);
    const sid = sigils.fresh();
    sigils.remember(said.blooms, sid);
    expect(sigils.continuing(said.blooms)).toBe(sid);
    expect(sigils.continuing(fromPrompt([{ role: "system", content: "other words" }, ...loop.slice(2)]).blooms)).toBe(sid);
    sigils.begin(sid);
    expect(sigils.continuing(said.blooms)).not.toBe(sid);
    sigils.end(sid);
    sigils.remember([{ content: [{ kind: "text", text: "b" }] }], "b");
    sigils.remember([{ content: [{ kind: "text", text: "c" }] }], "c");
    sigils.remember([{ content: [{ kind: "text", text: "d" }] }], "d");
    expect(forgot).toContain("b");
  });

  test("with no tools, by its last two turns, reasoning and spacing aside", () => {
    const sigils = new Sigils(() => {});
    sigils.remember([{ content: [{ kind: "text", text: "hi  there" }], grew: [{ kind: "sepal", text: "x" }, { kind: "text", text: "yo" }] }], "s-1");
    expect(sigils.continuing([{ content: [{ kind: "text", text: "hi there" }], grew: [{ kind: "text", text: "yo " }] }])).toBe("s-1");
    expect(sigils.continuing([{ content: [{ kind: "text", text: "hi there" }], grew: [{ kind: "text", text: "yo " }, { kind: "tendril", callId: "n1", name: "note" }, { kind: "grasp", callId: "n1", result: "news" }] }])).toBe("s-1");
    expect(sigils.continuing([{ content: [{ kind: "text", text: "hi there" }], grew: [{ kind: "text", text: "hey" }] }])).not.toBe("s-1");
  });

  test("only from where it last left off: an earlier copy starts its own", () => {
    const sigils = new Sigils(() => {});
    const turn = (id: string) => [{ kind: "tendril", callId: id, name: "f" }, { kind: "grasp", callId: id, result: "r" }];
    sigils.remember([{ content: [{ kind: "text", text: "go" }], grew: [...turn("t1"), ...turn("t2"), { kind: "text", text: "done" }] }], "s-1");
    expect(sigils.continuing([{ content: [{ kind: "text", text: "go" }], grew: [...turn("t2"), ...turn("t1"), { kind: "text", text: "done" }] }])).toBe("s-1");
    expect(sigils.continuing([{ content: [{ kind: "text", text: "go" }], grew: [...turn("t1"), { kind: "text", text: "fine" }] }])).not.toBe("s-1");
    const one = { content: [{ kind: "text", text: "hi" }], grew: [{ kind: "text", text: "hello" }] };
    const two = { content: [{ kind: "text", text: "joke" }], grew: [{ kind: "text", text: "bee" }] };
    sigils.remember([one], "s-2");
    sigils.remember([one, two], "s-2");
    expect(sigils.continuing([one])).not.toBe("s-2");
    expect(sigils.continuing([one, two])).toBe("s-2");
  });

  test("a line an app names is one sigil", () => {
    const sigils = new Sigils(() => {});
    const mine = sigils.named("window-1");
    expect(sigils.named("window-1")).toBe(mine);
    expect(sigils.named("window-2")).not.toBe(mine);
    sigils.begin(mine);
    expect(sigils.named("window-1")).not.toBe(mine);
  });
});

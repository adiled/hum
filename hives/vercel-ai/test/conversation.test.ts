// An AI SDK prompt read as blooms, and what a call opens with.

import { describe, expect, test } from "bun:test";
import type { LanguageModelV3Prompt } from "@ai-sdk/provider";
import { fromPrompt, opening, Sigils, trace } from "../src/conversation.ts";

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
  test("a conversation that only grows keeps its sigil, and one edited or growing gets its own", () => {
    const forgot: string[] = [];
    const sigils = new Sigils((sid) => forgot.push(sid), 2);
    const said = fromPrompt(loop);
    const sid = sigils.fresh();
    sigils.remember(said.system, said.blooms, sid);
    expect(sigils.continuing(said.system, said.blooms)).toBe(sid);
    expect(sigils.continuing("other words", said.blooms)).not.toBe(sid);
    sigils.begin(sid);
    expect(sigils.continuing(said.system, said.blooms)).not.toBe(sid);
    sigils.end(sid);
    sigils.remember(undefined, [{ content: [{ kind: "text", text: "b" }] }], "b");
    sigils.remember(undefined, [{ content: [{ kind: "text", text: "c" }] }], "c");
    expect(forgot).toEqual([sid]);
  });

  test("reasoning and spacing leave the trace alone", () => {
    const a = trace("s", [{ content: [{ kind: "text", text: "hi  there" }], grew: [{ kind: "sepal", text: "x" }, { kind: "text", text: "yo" }] }]);
    const b = trace(" s ", [{ content: [{ kind: "text", text: "hi there" }], grew: [{ kind: "text", text: "yo " }] }]);
    expect(a).toBe(b);
  });
});

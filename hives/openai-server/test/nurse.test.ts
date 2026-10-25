// The nurse half's reading of a prompt: its graft as OpenAI messages, a
// plain-text graft to grow on from, and how the bloom grows as the
// request's settings.

import { describe, expect, test } from "bun:test";
import { askOf, messagesOf, plainText } from "../src/nurse.ts";

describe("the nurse half", () => {
  test("a graft is the conversation, tool calls and answers in their places", () => {
    expect(messagesOf([{
      content: [{ kind: "text", text: "list" }],
      grew: [
        { kind: "sepal", text: "hmm" },
        { kind: "tendril", callId: "c1", name: "ls", args: { dir: "." } },
        { kind: "grasp", callId: "c1", result: "a.txt" },
        { kind: "text", text: "There is a.txt." },
      ],
    }])).toEqual([
      { role: "user", content: "list" },
      { role: "assistant", content: null, tool_calls: [{ id: "c1", type: "function", function: { name: "ls", arguments: '{"dir":"."}' } }] },
      { role: "tool", tool_call_id: "c1", content: "a.txt" },
      { role: "assistant", content: "There is a.txt." },
    ]);
  });

  test("only a graft no one prompted is plain text to grow on from", () => {
    expect(plainText([{ grew: [{ kind: "text", text: "def f():" }] }])).toBe("def f():");
    expect(plainText([{ content: [{ kind: "text", text: "hi" }], grew: [{ kind: "text", text: "hello" }] }])).toBeUndefined();
    expect(plainText([])).toBeUndefined();
  });

  test("how a bloom grows becomes the request's settings", () => {
    expect(askOf({
      chi: "prompt",
      temperature: 0.2,
      prune: { p: 0.9 },
      season: 64,
      frost: ["END"],
      seed: 3,
      trellis: { type: "object" },
      tendrils: { reach: "readFile", parallel: false },
      sepal: { effort: "high" },
      ext: { "openai-server": { sampling: { presence_penalty: 0.4 } } },
    })).toEqual({
      temperature: 0.2,
      top_p: 0.9,
      max_completion_tokens: 64,
      stop: ["END"],
      seed: 3,
      response_format: { type: "json_schema", json_schema: { name: "trellis", schema: { type: "object" } } },
      tool_choice: { type: "function", function: { name: "readFile" } },
      parallel_tool_calls: false,
      reasoning_effort: "high",
      presence_penalty: 0.4,
    });
  });
});

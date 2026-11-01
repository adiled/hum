// The nurse half's reading of a prompt: its graft as OpenAI messages, tool
// answers a caller added to a conversation the nurse kept, a plain-text
// graft to grow on from, and how the bloom grows as the request's settings.

import { describe, expect, test } from "bun:test";
import { askOf, lateAnswers, messagesOf, plainText } from "../src/nurse.ts";

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

  test("tool answers a caller added to the last bloom join the kept conversation, once", () => {
    const kept = [
      { role: "user", content: "Start the build" },
      { role: "assistant", content: "Started it as job 17." },
    ];
    const delivered = {
      content: [{ kind: "text", text: "Start the build" }],
      grew: [
        { kind: "text", text: "Started it as job 17." },
        { kind: "tendril", callId: "check_17", name: "check", args: { id: 17 } },
        { kind: "grasp", callId: "check_17", result: "job 17 finished" },
      ],
    };
    const late = lateAnswers(kept, delivered);
    expect(late).toEqual([
      { role: "assistant", content: null, tool_calls: [{ id: "check_17", type: "function", function: { name: "check", arguments: '{"id":17}' } }] },
      { role: "tool", tool_call_id: "check_17", content: "job 17 finished" },
    ]);
    expect(lateAnswers([...kept, ...late], delivered)).toEqual([]);
  });

  test("a late answer to the nurse's own call joins the kept call, which is not said twice", () => {
    const kept = [
      { role: "user", content: "read x" },
      { role: "assistant", content: null, tool_calls: [{ id: "call_1", type: "function", function: { name: "readFile", arguments: "{}" } }] },
    ];
    const answered = { grew: [{ kind: "tendril", callId: "call_1", name: "readFile", args: {} }, { kind: "grasp", callId: "call_1", result: "X" }] };
    expect(lateAnswers(kept, answered)).toEqual([{ role: "tool", tool_call_id: "call_1", content: "X" }]);
    expect(lateAnswers(kept, { grew: [{ kind: "grasp", callId: "nobody", result: "?" }] })).toEqual([]);
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

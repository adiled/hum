// An Anthropic conversation read as blooms, what a request opens with,
// and the sigil it keeps.

import { describe, expect, test } from "bun:test";
import { answers, fromMessages, opening, Sigils, systemOf, trace } from "../src/conversation.ts";
import { stopReasonOf, usageOf } from "../src/growth.ts";

describe("fromMessages", () => {
  test("each user turn opens a bloom, and what the assistant did grows in it", () => {
    const said = fromMessages([{ type: "text", text: "be terse" }, { type: "text", text: "and kind" }], [
      { role: "user", content: "list" },
      {
        role: "assistant",
        content: [
          { type: "thinking", thinking: "hmm", signature: "sig" },
          { type: "redacted_thinking", data: "opaque" },
          { type: "tool_use", id: "c1", name: "ls", input: { dir: "." } },
        ],
      },
      { role: "user", content: [{ type: "tool_result", tool_use_id: "c1", content: [{ type: "text", text: "a.txt" }, { type: "text", text: "b.txt" }] }] },
      { role: "assistant", content: "There are two." },
      { role: "system", content: "and brief" },
      { role: "user", content: [{ type: "text", text: "and this?" }, { type: "image", source: { type: "base64", media_type: "image/png", data: "AAAA" } }] },
    ]);
    expect(said.system).toBe("be terse\n\nand kind\n\nand brief");
    expect(said.tail).toBe("user");
    expect(said.blooms.length).toBe(2);
    expect(said.blooms[0].grew).toEqual([
      { kind: "sepal", text: "hmm", propolis: "sig" },
      { kind: "tendril", callId: "c1", name: "ls", args: { dir: "." } },
      { kind: "grasp", callId: "c1", result: "a.txt\nb.txt" },
      { kind: "text", text: "There are two." },
    ]);
    const { graft, content } = opening(said);
    expect(graft.length).toBe(1);
    expect(content).toEqual([{ kind: "text", text: "and this?" }, { kind: "pollen", grain: { kind: "image", mediaType: "image/png", data: "AAAA" } }]);
  });

  test("a conversation ending in tool results grows on, every bloom its graft", () => {
    const said = fromMessages(undefined, [
      { role: "user", content: "go" },
      { role: "assistant", content: [{ type: "tool_use", id: "a", name: "f", input: {} }, { type: "tool_use", id: "b", name: "f", input: {} }] },
      { role: "user", content: [{ type: "tool_result", tool_use_id: "a", content: "A" }, { type: "tool_result", tool_use_id: "b", content: "B", is_error: true }] },
    ]);
    expect(said.tail).toBe("tool");
    expect(answers(said)).toEqual([{ callId: "a", result: "A" }, { callId: "b", result: "B", isError: true }]);
    expect(opening(said)).toEqual({ graft: said.blooms, content: [] });
  });

  test("words after tool results open the next turn", () => {
    const said = fromMessages(undefined, [
      { role: "user", content: "go" },
      { role: "assistant", content: [{ type: "tool_use", id: "a", name: "f", input: {} }] },
      { role: "user", content: [{ type: "tool_result", tool_use_id: "a", content: "A" }, { type: "text", text: "now this" }] },
    ]);
    expect(said.tail).toBe("user");
    expect(answers(said)).toEqual([]);
    expect(opening(said).content).toEqual([{ kind: "text", text: "now this" }]);
    expect(opening(said).graft[0].grew?.map((p) => p.kind)).toEqual(["tendril", "grasp"]);
  });

  test("what only Anthropic can read is left out", () => {
    const said = fromMessages(undefined, [{
      role: "user",
      content: [
        { type: "image", source: { type: "file", file_id: "file_1" } },
        { type: "document", source: { type: "file", file_id: "file_2" } },
        { type: "text", text: "hi" },
      ],
    }]);
    expect(opening(said).content).toEqual([{ kind: "text", text: "hi" }]);
  });

  test("a system of blocks is their words, a string is itself, and nothing is none", () => {
    expect(systemOf([{ type: "text", text: "one" }, { type: "text", text: "two" }])).toBe("one\n\ntwo");
    expect(systemOf("plain")).toBe("plain");
    expect(systemOf(undefined)).toBeUndefined();
    expect(systemOf([])).toBeUndefined();
  });
});

describe("a conversation keeps its sigil", () => {
  test("the trace reads what a client sends back, whitespace and thinking aside", () => {
    const a = fromMessages(undefined, [{ role: "user", content: "hi" }, { role: "assistant", content: "hello  there" }]);
    const b = fromMessages(undefined, [{ role: "user", content: "hi" }, { role: "assistant", content: [{ type: "thinking", thinking: "x", signature: "s" }, { type: "text", text: "hello there\n" }] }]);
    expect(trace("default", undefined, a.blooms)).toBe(trace("default", undefined, b.blooms));
    expect(trace("default", undefined, a.blooms)).not.toBe(trace("acme", undefined, a.blooms));
  });

  test("a sigil is continued only while it is not growing, and only by its tenant", () => {
    const forgot: string[] = [];
    const sigils = new Sigils((sid) => forgot.push(sid), 1);
    const blooms = fromMessages(undefined, [{ role: "user", content: "hi" }, { role: "assistant", content: "hello" }]).blooms;
    expect(sigils.continuing("default", undefined, [])).not.toBe(sigils.continuing("default", undefined, []));
    expect(sigils.fresh("acme")).toStartWith("acme:");
    sigils.remember("default", undefined, blooms, "s-1");
    expect(sigils.continuing("default", undefined, blooms)).toBe("s-1");
    expect(sigils.continuing("acme", undefined, blooms)).not.toBe("s-1");
    sigils.begin("s-1");
    expect(sigils.continuing("default", undefined, blooms)).not.toBe("s-1");
    sigils.end("s-1");
    sigils.remember("default", undefined, [...blooms, ...blooms], "s-2");
    expect(forgot).toEqual(["s-1"]);
  });
});

describe("a wilt, said the Anthropic way", () => {
  test("its reason is a stop_reason", () => {
    expect(["stop", "success", undefined, "length", "max_tokens", "refusal"].map(stopReasonOf)).toEqual(["end_turn", "end_turn", "end_turn", "max_tokens", "max_tokens", "refusal"]);
  });

  test("its nectar is usage, the counts Anthropic names and no others", () => {
    expect(usageOf({ input_tokens: 3, output_tokens: 2, cache_read_input_tokens: 1, cost: "x" })).toEqual({ input_tokens: 3, output_tokens: 2, cache_read_input_tokens: 1 });
    expect(usageOf(undefined)).toEqual({ input_tokens: 0, output_tokens: 0 });
  });
});

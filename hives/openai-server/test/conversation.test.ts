// An OpenAI conversation read as blooms, and what a request opens with.

import { describe, expect, test } from "bun:test";
import { answers, fromChat, fromResponses, opening, Sigils, trace } from "../src/conversation.ts";

describe("fromChat", () => {
  test("each user turn opens a bloom, and what the assistant did grows in it", () => {
    const said = fromChat([
      { role: "system", content: "be terse" },
      { role: "developer", content: "and kind" },
      { role: "user", content: "list" },
      { role: "assistant", content: null, tool_calls: [{ id: "c1", type: "function", function: { name: "ls", arguments: '{"dir":"."}' } }] },
      { role: "tool", tool_call_id: "c1", content: "a.txt" },
      { role: "assistant", content: "There is a.txt." },
      { role: "user", content: [{ type: "text", text: "and this?" }, { type: "image_url", image_url: { url: "data:image/png;base64,AAAA" } }] },
    ]);
    expect(said.system).toBe("be terse\n\nand kind");
    expect(said.tail).toBe("user");
    expect(said.blooms.length).toBe(2);
    expect(said.blooms[0].grew).toEqual([
      { kind: "tendril", callId: "c1", name: "ls", args: { dir: "." } },
      { kind: "grasp", callId: "c1", result: "a.txt" },
      { kind: "text", text: "There is a.txt." },
    ]);
    const { graft, content } = opening(said);
    expect(graft.length).toBe(1);
    expect(content[1]).toEqual({ kind: "pollen", grain: { kind: "image", mediaType: "image/png", data: "AAAA" } });
  });

  test("a conversation ending in tool answers grows on, every bloom its graft", () => {
    const said = fromChat([
      { role: "user", content: "go" },
      { role: "assistant", tool_calls: [{ id: "a", function: { name: "f", arguments: "{}" } }, { id: "b", function: { name: "f", arguments: "{}" } }] },
      { role: "tool", tool_call_id: "a", content: "A" },
      { role: "tool", tool_call_id: "b", content: [{ type: "text", text: "B" }] },
    ]);
    expect(said.tail).toBe("tool");
    expect(answers(said).map((a) => [a.callId, a.result])).toEqual([["a", "A"], ["b", "B"]]);
    expect(opening(said)).toEqual({ graft: said.blooms, content: [] });
  });
});

describe("fromResponses", () => {
  test("items, instructions and a predecessor's blooms read alike", () => {
    const before = fromResponses("one");
    before.blooms[0].grew = [{ kind: "text", text: "ONE" }];
    const said = fromResponses([
      { role: "developer", content: "rules" },
      { type: "function_call", call_id: "c", name: "f", arguments: "{}" },
      { type: "function_call_output", call_id: "c", output: "out" },
      { type: "message", role: "user", content: [{ type: "input_text", text: "two" }] },
    ], "be brief", before);
    expect(said.system).toBe("be brief\n\nrules");
    expect(said.blooms.length).toBe(2);
    expect(said.blooms[0].grew?.map((p) => p.kind)).toEqual(["text", "tendril", "grasp"]);
    expect(opening(said).content).toEqual([{ kind: "text", text: "two" }]);
  });
});

describe("a conversation keeps its sigil", () => {
  test("the trace reads what a client sends back, whitespace aside", () => {
    const a = fromChat([{ role: "user", content: "hi" }, { role: "assistant", content: "hello  there" }]);
    const b = fromChat([{ role: "user", content: "hi" }, { role: "assistant", content: "hello there\n" }]);
    expect(trace("key-a", undefined, a.blooms)).toBe(trace("key-a", undefined, b.blooms));
    expect(trace("key-a", undefined, a.blooms)).not.toBe(trace("key-b", undefined, a.blooms));
  });

  test("the trace reads a turn kept as one message as the steps it grew in", () => {
    const call = (id: string) => ({ id, type: "function", function: { name: "zsh", arguments: "{}" } });
    const stepped = fromChat([
      { role: "user", content: "check it" },
      { role: "assistant", content: "I'll look.", tool_calls: [call("c1")] },
      { role: "tool", tool_call_id: "c1", content: "a" },
      { role: "assistant", content: "Then this.", tool_calls: [call("c2")] },
      { role: "tool", tool_call_id: "c2", content: "b" },
      { role: "assistant", content: "Done." },
    ]);
    const kept = fromChat([
      { role: "user", content: "check it" },
      { role: "assistant", content: "I'll look.Then this.\n\nDone.", tool_calls: [call("c1"), call("c2")] },
      { role: "tool", tool_call_id: "c1", content: "a" },
      { role: "tool", tool_call_id: "c2", content: "b" },
    ]);
    expect(trace("key-a", undefined, kept.blooms)).toBe(trace("key-a", undefined, stepped.blooms));
    const reordered = fromChat([
      { role: "user", content: "check it" },
      { role: "assistant", content: "I'll look.Then this.Done.", tool_calls: [call("c2"), call("c1")] },
      { role: "tool", tool_call_id: "c2", content: "b" },
      { role: "tool", tool_call_id: "c1", content: "a" },
    ]);
    expect(trace("key-a", undefined, reordered.blooms)).not.toBe(trace("key-a", undefined, stepped.blooms));
    const answered = fromChat([
      { role: "user", content: "check it" },
      { role: "assistant", content: "I'll look.Then this.Done.", tool_calls: [call("c1"), call("c2"), call("check_17")] },
      { role: "tool", tool_call_id: "c1", content: "a" },
      { role: "tool", tool_call_id: "c2", content: "b" },
      { role: "tool", tool_call_id: "check_17", content: "job 17 finished" },
    ]);
    expect(trace("key-a", undefined, answered.blooms)).not.toBe(trace("key-a", undefined, stepped.blooms));
  });

  test("a sigil is continued only while it is not growing, and only by its caller", () => {
    const forgot: string[] = [];
    const sigils = new Sigils((sid) => forgot.push(sid), 1);
    const blooms = fromChat([{ role: "user", content: "hi" }, { role: "assistant", content: "hello" }]).blooms;
    expect(sigils.continuing("key-a", undefined, [])).not.toBe(sigils.continuing("key-a", undefined, []));
    sigils.remember("key-a", undefined, blooms, "oai-1");
    expect(sigils.continuing("key-a", undefined, blooms)).toBe("oai-1");
    expect(sigils.continuing("key-b", undefined, blooms)).not.toBe("oai-1");
    sigils.begin("oai-1");
    expect(sigils.continuing("key-a", undefined, blooms)).not.toBe("oai-1");
    sigils.end("oai-1");
    sigils.remember("key-a", undefined, [...blooms, ...blooms], "oai-2");
    expect(forgot).toEqual(["oai-1"]);
  });
});

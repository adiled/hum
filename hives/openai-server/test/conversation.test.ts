// An OpenAI conversation read as blooms, and what a request opens with.

import { describe, expect, test } from "bun:test";
import { answers, fromChat, fromResponses, opening, Sigils } from "../src/conversation.ts";

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

describe("a tool the server ran", () => {
  test("is read back from a chat message and from items, as what grew", () => {
    const lookup = { id: "mcp-1", name: "lookup", arguments: '{"q":"bees"}', output: "found bees", error: null };
    const echoed = fromChat([
      { role: "user", content: "find bees" },
      { role: "assistant", content: "after tools: found bees", mcp_calls: [lookup] },
    ]);
    const grew = [
      { kind: "tendril", callId: "mcp-1", name: "lookup", args: { q: "bees" } },
      { kind: "grasp", callId: "mcp-1", result: "found bees", isError: false },
      { kind: "text", text: "after tools: found bees" },
    ];
    expect(echoed.blooms[0].grew).toEqual(grew);
    const items = fromResponses([
      { role: "user", content: "find bees" },
      { type: "mcp_call", ...lookup },
      { type: "message", role: "assistant", content: [{ type: "output_text", text: "after tools: found bees" }] },
    ]);
    expect(items.blooms[0].grew).toEqual(grew);
  });
});

const call = (id: string, name = "zsh") => ({ id, type: "function", function: { name, arguments: "{}" } });

describe("a conversation keeps its sigil", () => {
  const stepped = [
    { role: "user", content: "check it" },
    { role: "assistant", content: "I'll look.", tool_calls: [call("c1")] },
    { role: "tool", tool_call_id: "c1", content: "a" },
    { role: "assistant", content: "Then this.", tool_calls: [call("c2")] },
    { role: "tool", tool_call_id: "c2", content: "b" },
    { role: "assistant", content: "Done." },
  ];
  const grown = () => {
    const sigils = new Sigils(() => {});
    sigils.remember("key-a", fromChat(stepped).blooms, "oai-1");
    return sigils;
  };
  const next = { role: "user", content: "and now?" };
  const asked = (messages: Array<Record<string, unknown>>) => opening(fromChat(messages as never)).graft;

  test("though old tool pairs are dropped", () => {
    const kept = [
      { role: "user", content: "check it" },
      { role: "assistant", content: "I'll look.Then this.", tool_calls: [call("c2")] },
      { role: "tool", tool_call_id: "c2", content: "b" },
      { role: "assistant", content: "Done." },
      next,
    ];
    expect(grown().continuing("key-a", asked(kept))).toBe("oai-1");
  });

  test("though a turn comes back as one message, its words run together", () => {
    const squashed = [
      { role: "user", content: "check it" },
      { role: "assistant", content: "I'll look.\nThen this.\n\nDone.", tool_calls: [call("c1"), call("c2")] },
      { role: "tool", tool_call_id: "c1", content: "a" },
      { role: "tool", tool_call_id: "c2", content: "b" },
      next,
    ];
    expect(grown().continuing("key-a", asked(squashed))).toBe("oai-1");
  });

  test("though the words said between calls are missing in one request and back in the next", () => {
    const sigils = new Sigils(() => {});
    const midTurn = fromChat([
      { role: "user", content: "check it" },
      { role: "assistant", content: null, tool_calls: [call("c1")] },
      { role: "tool", tool_call_id: "c1", content: "a" },
      { role: "assistant", content: null, tool_calls: [call("c2")] },
      { role: "tool", tool_call_id: "c2", content: "b" },
    ]);
    sigils.remember("key-a", [{ ...midTurn.blooms[0], grew: [...(midTurn.blooms[0].grew ?? []), { kind: "text", text: "Done." }] }], "oai-1");
    expect(sigils.continuing("key-a", asked([...stepped, next]))).toBe("oai-1");
  });

  test("though the system prompt changes", () => {
    const told = fromChat([{ role: "system", content: "[bookkeeping] deem it" }, ...stepped, next] as never);
    expect(told.system).toBe("[bookkeeping] deem it");
    expect(grown().continuing("key-a", opening(told).graft)).toBe("oai-1");
  });

  test("though a call and its answer are made up to hand the model news", () => {
    const delivered = [...stepped, { role: "assistant", content: null, tool_calls: [call("check_17", "check")] }, { role: "tool", tool_call_id: "check_17", content: "job 17 finished" }];
    expect(grown().continuing("key-a", asked(delivered))).toBe("oai-1");
  });

  test("with no tools, by its last two turns, older turns compacted away", () => {
    const sigils = new Sigils(() => {});
    const chat = [
      { role: "user", content: "hi" },
      { role: "assistant", content: "Hello!" },
      { role: "user", content: "tell me a joke" },
      { role: "assistant", content: "Why did the bee..." },
      { role: "user", content: "another" },
      { role: "assistant", content: "A wasp walks in..." },
    ];
    sigils.remember("key-a", fromChat(chat).blooms, "oai-1");
    expect(sigils.continuing("key-a", asked([...chat, next]))).toBe("oai-1");
    expect(sigils.continuing("key-a", asked([{ role: "system", content: "[summary] we said hi" }, ...chat.slice(2), next]))).toBe("oai-1");
    const delivered = [...chat, { role: "assistant", content: null, tool_calls: [call("note_1", "note")] }, { role: "tool", tool_call_id: "note_1", content: "a note" }];
    expect(sigils.continuing("key-a", asked(delivered))).toBe("oai-1");
    expect(sigils.continuing("key-a", asked([...chat.slice(0, 5), { role: "assistant", content: "A hornet walks in..." }, next]))).not.toBe("oai-1");
  });

  test("though the calls of a squashed turn come back in another order", () => {
    const reordered = [
      { role: "user", content: "check it" },
      { role: "assistant", content: "I'll look. Then this. Done.", tool_calls: [call("c2"), call("c1")] },
      { role: "tool", tool_call_id: "c2", content: "b" },
      { role: "tool", tool_call_id: "c1", content: "a" },
      next,
    ];
    expect(grown().continuing("key-a", asked(reordered))).toBe("oai-1");
  });

  test("though its last call was one the server ran, and the client dropped it and reshaped the words", () => {
    const lookup = { id: "m1", name: "lookup", arguments: "{}", output: "found bees", error: null };
    const turn = [...stepped, { role: "user", content: "find bees" }, { role: "assistant", content: "Looking.\nFound bees.", mcp_calls: [lookup] }];
    const back = [...stepped, { role: "user", content: "find bees" }, { role: "assistant", content: "Found bees." }, next];
    const sigils = new Sigils(() => {});
    sigils.remember("key-a", fromChat(turn as never).blooms, "oai-1", ["m1"]);
    expect(sigils.continuing("key-a", asked(back))).toBe("oai-1");
    const unaware = new Sigils(() => {});
    unaware.remember("key-a", fromChat(turn as never).blooms, "oai-1");
    expect(unaware.continuing("key-a", asked(back))).not.toBe("oai-1");
  });

  test("only from where it last left off: a copy taken before its newest call starts its own", () => {
    const copy = [
      { role: "user", content: "check it" },
      { role: "assistant", content: "I'll look.", tool_calls: [call("c1")] },
      { role: "tool", tool_call_id: "c1", content: "a" },
      { role: "assistant", content: "Looks fine." },
      next,
    ];
    expect(grown().continuing("key-a", asked(copy))).not.toBe("oai-1");
  });

  test("only from where it last left off: a tool-less copy taken before its latest turn starts its own", () => {
    const sigils = new Sigils(() => {});
    const chat = [
      { role: "user", content: "hi" },
      { role: "assistant", content: "Hello!" },
      { role: "user", content: "tell me a joke" },
      { role: "assistant", content: "Why did the bee..." },
      { role: "user", content: "another" },
      { role: "assistant", content: "A wasp walks in..." },
    ];
    sigils.remember("key-a", fromChat(chat.slice(0, 4)).blooms, "oai-1");
    expect(sigils.continuing("key-a", asked([...chat.slice(0, 4), next]))).toBe("oai-1");
    sigils.remember("key-a", fromChat(chat).blooms, "oai-1");
    expect(sigils.continuing("key-a", asked([...chat.slice(0, 4), next]))).not.toBe("oai-1");
    expect(sigils.continuing("key-a", asked([...chat, next]))).toBe("oai-1");
  });

  test("never for a stranger, nor while it is growing", () => {
    const sigils = grown();
    expect(sigils.continuing("key-b", asked([...stepped, next]))).not.toBe("oai-1");
    sigils.begin("oai-1");
    expect(sigils.continuing("key-a", asked([...stepped, next]))).not.toBe("oai-1");
    sigils.end("oai-1");
    expect(sigils.continuing("key-a", asked([...stepped, next]))).toBe("oai-1");
    expect(sigils.continuing("key-a", [])).not.toBe(sigils.continuing("key-a", []));
  });

  test("a line a client names is its own, apart from every other caller's", () => {
    const sigils = new Sigils(() => {});
    const mine = sigils.named("key-a", "window-1");
    expect(sigils.named("key-a", "window-1")).toBe(mine);
    expect(sigils.named("key-b", "window-1")).not.toBe(mine);
    sigils.begin(mine);
    expect(sigils.named("key-a", "window-1")).not.toBe(mine);
  });

  test("a sigil no key finds any more is forgotten", () => {
    const forgot: string[] = [];
    const sigils = new Sigils((sid) => forgot.push(sid), 2);
    sigils.remember("key-a", fromChat([{ role: "user", content: "hi" }, { role: "assistant", content: "hello" }]).blooms, "oai-1");
    sigils.remember("key-a", fromChat([{ role: "user", content: "yo" }, { role: "assistant", content: "hey" }]).blooms, "oai-2");
    expect(forgot).toEqual([]);
    sigils.remember("key-a", fromChat([{ role: "user", content: "sup" }, { role: "assistant", content: "hi" }]).blooms, "oai-3");
    expect(forgot).toEqual(["oai-1"]);
  });
});

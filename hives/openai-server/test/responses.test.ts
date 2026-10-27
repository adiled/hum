// The Responses API's store: what it keeps for each key, the latest of
// each conversation among it, and no more than it can hold.

import { describe, expect, test } from "bun:test";
import { Shelf, type Kept } from "../src/responses.ts";

const kept = (label: string, sid: string): Kept => ({ label, sid, said: { blooms: [], tail: "assistant" }, response: {} });

describe("the Responses store", () => {
  test("a conversation follows its latest response, for its own key", () => {
    const shelf = new Shelf();
    shelf.put("resp_1", kept("key-a", "oai-1"), "thread");
    shelf.put("resp_2", kept("key-a", "oai-1"), "thread");
    expect(shelf.latest("key-a", "thread")?.sid).toBe("oai-1");
    expect(shelf.get("key-a", "resp_1")).toBeDefined();
    expect(shelf.latest("key-b", "thread")).toBeUndefined();
    expect(shelf.get("key-b", "resp_2")).toBeUndefined();
  });

  test("forgets a conversation when it forgets the response the conversation was at", () => {
    const shelf = new Shelf(2);
    for (let i = 0; i < 10; i++) shelf.put(`resp_${i}`, kept("key-a", `oai-${i}`), `thread-${i}`);
    expect(shelf.size).toEqual({ responses: 2, conversations: 2 });
    expect(shelf.latest("key-a", "thread-0")).toBeUndefined();
    expect(shelf.latest("key-a", "thread-9")?.sid).toBe("oai-9");
  });

  test("keeps a conversation while its latest response is kept", () => {
    const shelf = new Shelf(2);
    shelf.put("resp_1", kept("key-a", "oai-1"), "thread");
    shelf.put("resp_2", kept("key-a", "oai-1"), "thread");
    shelf.put("resp_3", kept("key-a", "oai-3"), "other");
    expect(shelf.latest("key-a", "thread")?.sid).toBe("oai-1");
    expect(shelf.size).toEqual({ responses: 2, conversations: 2 });
  });

  test("a response deleted takes its conversation's place with it", () => {
    const shelf = new Shelf();
    shelf.put("resp_1", kept("key-a", "oai-1"), "thread");
    expect(shelf.drop("key-b", "resp_1")).toBe(false);
    expect(shelf.drop("key-a", "resp_1")).toBe(true);
    expect(shelf.latest("key-a", "thread")).toBeUndefined();
    expect(shelf.size).toEqual({ responses: 0, conversations: 0 });
  });
});

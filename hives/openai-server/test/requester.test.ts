// A caller's bee holding the tools its bloom reached it for: its link
// stays open while it holds one, a hold lasts ten minutes, and a hold
// let go unanswered cancels its bloom. The requester here never reaches a
// humd; what it sends is caught, and what it hears is played to it.

import { afterEach, describe, expect, setSystemTime, test } from "bun:test";
import type { Tone } from "../../../thrum-clients/ts/index.ts";
import { grow } from "../src/growth.ts";
import { prompt } from "../src/hive.ts";
import { Requester } from "../src/requester.ts";

const MINUTE = 60_000;
const nowhere = "/nonexistent/hum";

afterEach(() => setSystemTime());

function caught(label: string): { rq: Requester; sent: Tone[]; hear: (t: Record<string, unknown>) => void; cancels: () => string[] } {
  const rq = new Requester(label, nowhere);
  const sent: Tone[] = [];
  const heard = new Map<string, (t: Tone) => unknown>();
  rq.bee.send = (t) => void sent.push(t);
  rq.bee.onSid = (sid, fn) => (heard.set(sid, fn), rq.bee);
  return {
    rq,
    sent,
    hear: (t) => void heard.get(t.sid as string)?.(t as unknown as Tone),
    cancels: () => sent.filter((t) => t.chi === "cancel").map((t) => t.sid ?? ""),
  };
}

describe("a requester holding a call", () => {
  test("is not idle while the hold is live, and is once it lapses", () => {
    const rq = new Requester("key-held", nowhere);
    const at = Date.now();
    rq.lastUsed = at - 5 * MINUTE;
    rq.hold("oai-1", "call_1");
    expect(rq.idle(at + 6 * MINUTE)).toBe(false);
    expect(rq.idle(at + 9 * MINUTE)).toBe(false);
    expect(rq.idle(at + 11 * MINUTE)).toBe(true);
    rq.close();
  });

  test("resumes the bloom for an answer within ten minutes, and not after", () => {
    const rq = new Requester("key-late", nowhere);
    const at = Date.now();
    rq.hold("oai-2", "call_2");
    setSystemTime(new Date(at + 9 * MINUTE));
    expect(rq.heldSid(["call_2"])).toBe("oai-2");
    setSystemTime(new Date(at + 11 * MINUTE));
    expect(rq.heldSid(["call_2"])).toBeUndefined();
    rq.close();
  });

  test("an answered call is let go, and the link may then close idle", () => {
    const { rq, cancels } = caught("key-answered");
    const at = Date.now();
    rq.hold("oai-3", "call_3");
    rq.release(["call_3"]);
    expect(rq.heldSid(["call_3"])).toBeUndefined();
    expect(rq.idle(at + 11 * MINUTE)).toBe(true);
    rq.lapse(at + 11 * MINUTE);
    rq.close();
    expect(cancels()).toEqual([]);
  });
});

describe("a hold let go unanswered", () => {
  test("cancels its bloom once it lapses, and only once", () => {
    const { rq, cancels } = caught("key-lapse");
    const at = Date.now();
    rq.hold("oai-4", "call_a");
    rq.hold("oai-4", "call_b");
    rq.lapse(at + 9 * MINUTE);
    expect(cancels()).toEqual([]);
    rq.lapse(at + 11 * MINUTE);
    rq.lapse(at + 12 * MINUTE);
    expect(cancels()).toEqual(["oai-4"]);
    rq.close();
    expect(cancels()).toEqual(["oai-4"]);
  });

  test("cancels its bloom as the link closes", () => {
    const { rq, cancels } = caught("key-close");
    rq.hold("oai-5", "call_5");
    rq.close();
    expect(cancels()).toEqual(["oai-5"]);
  });

  test("leaves a bloom alone that a prompt has since opened on its sid, or that is growing", () => {
    const { rq, cancels } = caught("key-again");
    const at = Date.now();
    rq.hold("oai-6", "call_6");
    rq.send(prompt("oai-6", undefined, { content: [] }));
    rq.hold("oai-7", "call_7");
    rq.follow("oai-7", () => {});
    rq.lapse(at + 11 * MINUTE);
    expect(cancels()).toEqual([]);
    rq.close();
  });

  test("whose cancel's wilt is never taken for a later bloom's", () => {
    const { rq, hear, cancels } = caught("key-stilled");
    const at = Date.now();
    rq.hold("oai-8", "call_8");
    rq.lapse(at + 11 * MINUTE);
    expect(cancels()).toEqual(["oai-8"]);
    const got: string[] = [];
    rq.follow("oai-8", (t) => void got.push(`${t.chi}${t.chi === "wilt" ? `:${t.reason}` : ""}`));
    hear({ chi: "wilt", sid: "oai-8", reason: "cancel" });
    hear({ chi: "unfurl", sid: "oai-8", petal: 0, kind: "text", text: "hi" });
    hear({ chi: "wilt", sid: "oai-8", reason: "cancel" });
    expect(got).toEqual(["unfurl", "wilt:cancel"]);
    rq.close();
  });
});

describe("a bloom's holds", () => {
  test("a turn gone back to the caller holds its calls, and cancels the bloom once they lapse", async () => {
    const { rq, hear, cancels } = caught("key-turn");
    const at = Date.now();
    const grown = grow(rq, "oai-9", { prompt: prompt("oai-9", undefined, { content: [{ kind: "text", text: "go" }] }) }, { timeoutMs: 5_000 });
    hear({ chi: "tendril-reach", sid: "oai-9", callId: "call_9", name: "readFile", args: {} });
    expect((await grown).finish).toBe("tool_calls");
    expect(rq.heldSid(["call_9"])).toBe("oai-9");
    rq.lapse(at + 11 * MINUTE);
    expect(cancels()).toEqual(["oai-9"]);
    rq.close();
  });

  test("a bloom that broke off lets its calls go, with nothing left to cancel", async () => {
    const { rq, hear, cancels } = caught("key-broke");
    const at = Date.now();
    const grown = grow(rq, "oai-10", { prompt: prompt("oai-10", undefined, { content: [{ kind: "text", text: "go" }] }) }, { timeoutMs: 5_000 });
    hear({ chi: "tendril-reach", sid: "oai-10", callId: "call_10", name: "readFile", args: {} });
    hear({ chi: "error", sid: "oai-10", code: "upstream_error", message: "broke" });
    await expect(grown).rejects.toThrow("broke");
    expect(rq.heldSid(["call_10"])).toBeUndefined();
    rq.lapse(at + 11 * MINUTE);
    rq.close();
    expect(cancels()).toEqual([]);
  });
});

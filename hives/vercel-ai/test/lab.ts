// The lab's bees for the end-to-end tests, in one process: a nurse that
// grows `lab-model` by rule, and an attendant that serves `lookup`. Every
// prompt, tendril-result, cancel and cleanup the nurse hears is written to
// `LAB_LOG`, one tone a line, for a test to read.
//
// The nurse reads the prompt's words:
//   "fail …"            breaks off with an error
//   "slow …"            unfurls a tick every 50 ms until it is cancelled
//   "weather: A, B"     reaches the forager's `weather` once a city, all at
//                       once, then says what came back
//   "look up X"         reaches the attendant's `lookup`, then says it
//   "both: X"           reaches `lookup` and the forager's `weather` at once
//   anything else       a sepal, then "echo: <words>"
// A prompt with no words grows on from its graft's last grasps. Every
// reach that comes back is told to the forager by `tendril-info`, as a
// nurse built on hum-nest does.

import { appendFileSync } from "node:fs";
import { randomUUID } from "node:crypto";
import { Bee, Chi, PetalKind, Role, type Bloom, type Petal, type Tone } from "../../../thrum-clients/ts/index.ts";

const LOG = process.env.LAB_LOG ?? "";
const NECTAR = { input_tokens: 7, cache_read_input_tokens: 4, output_tokens: 5 };

const heard = (t: unknown) => appendFileSync(LOG, JSON.stringify(t) + "\n");

const nurse = new Bee({
  hive: "lab",
  as: "nurse",
  role: Role.nurse,
  hello: { models: ["lab-model"], headroom: { freeSlots: 8, totalSlots: 8 } },
});
const lookup = new Bee({
  hive: "lab",
  as: "lookup",
  role: Role.attendant,
  hello: { tools: [{ name: "lookup", description: "Look a thing up.", inputSchema: { type: "object", properties: { q: { type: "string" } } } }] },
});

interface Growing {
  petal: number;
  cancelled: boolean;
  waiting: Map<string, (t: Tone<"tendril-result">) => void>;
}

const sigils = new Map<string, Growing>();

function send(sid: string, tone: Record<string, unknown>): void {
  nurse.send({ ...tone, sid } as Tone);
}

function say(sid: string, g: Growing, kind: "text" | "sepal", text: string): void {
  const petal = g.petal++;
  send(sid, { chi: "unfurl", petal, kind, text });
  send(sid, { chi: "unfurl", petal, kind, closed: true });
}

function wilt(sid: string, reason = "end_turn"): void {
  send(sid, { chi: "wilt", reason, nectar: NECTAR });
}

async function reach(sid: string, g: Growing, name: string, args: unknown, unfurl: boolean): Promise<Tone<"tendril-result">> {
  const callId = `lab-${randomUUID()}`;
  if (unfurl) {
    const petal = g.petal++;
    send(sid, { chi: "unfurl", petal, kind: PetalKind.tendril, callId, name, sap: JSON.stringify(args) });
    send(sid, { chi: "unfurl", petal, kind: PetalKind.tendril, closed: true });
  }
  const back = new Promise<Tone<"tendril-result">>((r) => g.waiting.set(callId, r));
  send(sid, { chi: "tendril-reach", callId, name, args });
  const t = await back;
  send(sid, { chi: "tendril-info", callId, name, args, result: t.output ?? t.result, isError: t.isError === true });
  return t;
}

function words(petals: Petal[] | undefined): string {
  return (petals ?? []).filter((p) => p.kind === PetalKind.text).map((p) => p.text ?? "").join(" ");
}

function offered(t: Tone<"prompt">, name: string): boolean {
  return (t.tools ?? []).some((x) => (x as { name?: string }).name === name);
}

async function grow(t: Tone<"prompt">): Promise<void> {
  const sid = t.sid ?? "";
  const g: Growing = { petal: 0, cancelled: false, waiting: new Map() };
  sigils.set(sid, g);
  const said = words(t.content);

  if (!said) {
    const last: Bloom | undefined = t.graft?.at(-1);
    const grasps = (last?.grew ?? []).filter((p) => p.kind === PetalKind.grasp).map((p) => String(p.result ?? ""));
    if (grasps.length === 0) {
      send(sid, { chi: "error", message: "nothing to grow on from", code: "cannot-grow-on" });
      return;
    }
    say(sid, g, "text", `grafted: ${grasps.join(", ")}`);
    return wilt(sid);
  }
  if (said.startsWith("fail")) {
    send(sid, { chi: "error", message: "the lab cell broke", code: "lab" });
    return;
  }
  if (said.startsWith("slow")) {
    const petal = g.petal++;
    for (let i = 0; !g.cancelled && i < 200; i++) {
      send(sid, { chi: "unfurl", petal, kind: "text", text: `tick${i} ` });
      await new Promise((r) => setTimeout(r, 50));
    }
    return wilt(sid, g.cancelled ? "cancel" : "end_turn");
  }
  const weather = said.match(/^weather: (.+)$/);
  if (weather && offered(t, "weather")) {
    say(sid, g, "text", "checking");
    const cities = weather[1].split(",").map((c) => c.trim());
    const back = await Promise.all(cities.map((city) => reach(sid, g, "weather", { city }, true)));
    if (g.cancelled) return;
    say(sid, g, "text", cities.map((c, i) => `${c} ${back[i].isError ? "failed" : back[i].output}`).join("; "));
    return wilt(sid);
  }
  const both = said.match(/^both: (.+)$/);
  if (both && offered(t, "weather") && offered(t, "lookup")) {
    const [found, sky] = await Promise.all([reach(sid, g, "lookup", { q: both[1] }, false), reach(sid, g, "weather", { city: both[1] }, true)]);
    say(sid, g, "text", `${sky.output} and ${found.output}`);
    return wilt(sid);
  }
  const look = said.match(/^look up (.+)$/);
  if (look && offered(t, "lookup")) {
    const back = await reach(sid, g, "lookup", { q: look[1] }, false);
    say(sid, g, "text", `so: ${back.output}`);
    return wilt(sid);
  }
  say(sid, g, "sepal", "thinking");
  say(sid, g, "text", `echo: ${said}`);
  wilt(sid);
}

nurse.on(Chi.prompt, (t) => {
  heard(t);
  void grow(t);
});
nurse.on(Chi.tendrilResult, (t) => {
  heard(t);
  sigils.get(t.sid ?? "")?.waiting.get(t.callId)?.(t);
});
nurse.on(Chi.cancel, (t) => {
  heard(t);
  const g = sigils.get(t.sid ?? "");
  if (g) g.cancelled = true;
});
nurse.on(Chi.cleanup, (t) => heard(t));

lookup.on(Chi.tendrilReach, (t) => {
  const q = (t.args as { q?: string } | undefined)?.q ?? "";
  lookup.send({ chi: "tendril-result", callId: t.callId, ...(t.sid ? { sid: t.sid } : {}), output: `found ${q}` });
});

await Promise.all([nurse.start(), lookup.start()]);
console.log("lab nestled");
await Promise.all([nurse.closed(), lookup.closed()]).catch(() => {});

// A nurse for the end-to-end tests. It grows `claude-lab` by script and
// says what it heard at /_heard: every prompt, tendril-result and cancel,
// whole.
//
// It answers by the words its prompt opens with: ONE reaches readFile
// once, PAR reaches it twice at once, SLOW grows twenty ticks slowly, LEN
// stops for max_tokens, THINK reasons first and seals it, FAIL breaks off.
// A bloom whose reaches have all come back grows what came back, and so
// does a prompt with no words whose graft ends in grasps. Anything else is
// echoed.

import { randomUUID } from "node:crypto";
import { createServer } from "node:http";
import { Bee, Chi, PetalKind, Role, type Petal, type Tone } from "../../../thrum-clients/ts/index.ts";

const heard = { prompts: [] as Tone<"prompt">[], results: [] as Tone<"tendril-result">[], cancels: [] as string[] };
const blooms = new Map<string, { waiting: Set<string>; got: string[]; ticking?: ReturnType<typeof setInterval> }>();
const nectar = { input_tokens: 11, output_tokens: 7, cache_read_input_tokens: 4 };

const bee = new Bee({ hive: "lab", as: "lab-nurse", role: Role.nurse, hello: { models: ["claude-lab"] } });
const send = (t: Tone) => bee.send(t);
const words = (petals: Petal[] = []) => petals.filter((p) => p.kind === PetalKind.text).map((p) => p.text ?? "").join(" ");

function say(sid: string, text: string, reason = "stop", petal = 0): void {
  send({ chi: "unfurl", sid, petal, kind: PetalKind.text, text });
  send({ chi: "unfurl", sid, petal, kind: PetalKind.text, closed: true });
  send({ chi: "wilt", sid, reason, nectar });
}

function reach(sid: string, paths: string[]): void {
  const calls = paths.map((path) => ({ callId: `toolu_${randomUUID().replace(/-/g, "").slice(0, 20)}`, path }));
  blooms.set(sid, { waiting: new Set(calls.map((c) => c.callId)), got: [] });
  send({ chi: "unfurl", sid, petal: 0, kind: PetalKind.text, text: "let me look" });
  send({ chi: "unfurl", sid, petal: 0, kind: PetalKind.text, closed: true });
  for (const { callId, path } of calls) send({ chi: "tendril-reach", sid, callId, name: "readFile", args: { path } });
}

bee.on(Chi.prompt, (t) => {
  heard.prompts.push(t);
  const sid = t.sid ?? "";
  const said = words(t.content);
  if (!said) {
    const grasps = (t.graft?.at(-1)?.grew ?? []).filter((p) => p.kind === PetalKind.grasp);
    return say(sid, grasps.length > 0 ? `after tools: ${grasps.map((g) => g.result).join(" | ")}` : "grew on");
  }
  if (said.includes("FAIL")) return send({ chi: "error", sid, code: "lab-broke", message: "the lab nurse broke off" });
  if (said.includes("PAR")) return reach(sid, ["a", "b"]);
  if (said.includes("ONE")) return reach(sid, ["x"]);
  if (said.includes("LEN")) return say(sid, "cut sho", "max_tokens");
  if (said.includes("THINK")) {
    send({ chi: "unfurl", sid, petal: 0, kind: PetalKind.sepal, text: "pondering" });
    send({ chi: "unfurl", sid, petal: 0, kind: PetalKind.sepal, propolis: "sig-lab" });
    send({ chi: "unfurl", sid, petal: 0, kind: PetalKind.sepal, closed: true });
    return say(sid, "thought it through", "stop", 1);
  }
  if (said.includes("SLOW")) {
    let i = 0;
    const ticking = setInterval(() => {
      if (i === 20) {
        clearInterval(ticking);
        return say(sid, "done");
      }
      send({ chi: "unfurl", sid, petal: 0, kind: PetalKind.text, text: `tick${i++} ` });
    }, 200);
    blooms.set(sid, { waiting: new Set(), got: [], ticking });
    return;
  }
  say(sid, `echo: ${said}`);
});

bee.on(Chi.tendrilResult, (t) => {
  heard.results.push(t);
  const sid = t.sid ?? "";
  const bloom = blooms.get(sid);
  if (!bloom || !bloom.waiting.delete(t.callId)) return;
  bloom.got.push(typeof t.output === "string" ? t.output : JSON.stringify(t.result));
  if (bloom.waiting.size === 0) say(sid, `after tools: ${bloom.got.join(" | ")}`, "stop", 1);
});

bee.on(Chi.cancel, (t) => {
  const sid = t.sid ?? "";
  heard.cancels.push(sid);
  clearInterval(blooms.get(sid)?.ticking);
  send({ chi: "wilt", sid, reason: "cancel" });
});

const server = createServer((_, res) => {
  res.writeHead(200, { "Content-Type": "application/json" });
  res.end(JSON.stringify(heard));
});
await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
await bee.start();
console.log(`nurse nestled http://127.0.0.1:${(server.address() as { port: number }).port}`);
await bee.closed().catch(() => {});

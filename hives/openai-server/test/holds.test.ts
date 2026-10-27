// End to end: a tool call the server holds for a caller, when the key's
// link ends under it. The key's next link cancels the bloom that reached
// for it, so no nurse is left growing toward an answer no one will give.
// A nurse of the test's own, which grows nothing, hears every cancel humd
// hands its nurses.

import { afterAll, beforeAll, expect, test } from "bun:test";
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { join } from "node:path";
import OpenAI from "openai";
import { hum, type Hum } from "./hum.ts";
import { upstream, type Upstream } from "./upstream.ts";

let up: Upstream;
let h: Hum;
let nurse: ChildProcess;
let said = "";

async function until(what: string, ok: () => boolean, ms = 15_000): Promise<void> {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    if (ok()) return;
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error(`never: ${what}\n${h.log()}\n${said}`);
}

beforeAll(async () => {
  up = await upstream();
  h = await hum(up.url, { keys: ["holder"] });
  nurse = spawn("bun", [join(import.meta.dir, "cancel-nurse.ts")], { env: h.env, stdio: ["ignore", "pipe", "pipe"] });
  nurse.stdout?.on("data", (b) => (said += b));
  nurse.stderr?.on("data", (b) => (said += b));
  await until("the cancel nurse nestles", () => said.includes("cancels nestled"));
}, 120_000);

afterAll(async () => {
  nurse?.kill("SIGTERM");
  await h?.close();
  await up?.close();
});

test("a held call whose link ends has its bloom cancelled by the key's next link", async () => {
  const ai = new OpenAI({ baseURL: h.base, apiKey: h.keys.holder, maxRetries: 0, timeout: 20_000 });
  const readFile = { type: "function" as const, function: { name: "readFile", parameters: { type: "object", properties: {} } } };
  const asked = await ai.chat.completions.create({ model: "gpt-lab", tools: [readFile], messages: [{ role: "user", content: "ONE held" }] });
  expect(asked.choices[0].finish_reason).toBe("tool_calls");
  expect(said).not.toContain("cancel oai-");

  const link = spawnSync("pgrep", ["-f", "bee link --hive openai-server --as key-holder"], { encoding: "utf8" }).stdout.trim().split("\n").filter(Boolean);
  expect(link.length).toBe(1);
  process.kill(Number(link[0]), "SIGKILL");
  await until("the server sees the link end", () => h.log().includes("[openai-server] key-holder:"));
  expect(said).not.toContain("cancel oai-");

  await ai.models.list();
  await until("the bloom is cancelled", () => /cancel oai-/.test(said));
}, 60_000);

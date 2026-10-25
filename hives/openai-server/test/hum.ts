// A hum of its own for each test file: a humd from this checkout and this
// hive's server, under Node as it ships, and nothing of the person's.
//
// Everything lives under one fresh directory, short enough for a unix
// socket: hum's config, state, data, cache and runtime roots, so the humd,
// every `hum bee link` and every key it enters stay there. The hive is
// installed by its marker; keys are added through the server's own `key
// add`. An attendant bee, `lab-lookup`, serves one tool, `lookup`, for a
// bloom to reach in-server. Every child is given its environment
// explicitly, so none can reach a humd but this one.

import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const HIVE_DIR = resolve(import.meta.dir, "..");
const REPO = resolve(HIVE_DIR, "../..");
export const HUM = join(REPO, "target", "debug", "hum");
const HUMD = join(REPO, "target", "debug", "humd");

export interface Hum {
  root: string;
  env: Record<string, string>;
  base: string;
  keys: Record<string, string>;
  log: () => string;
  close: () => Promise<void>;
}

async function freePort(): Promise<number> {
  return new Promise((r) => {
    const s = createServer();
    s.listen(0, "127.0.0.1", () => {
      const port = (s.address() as { port: number }).port;
      s.close(() => r(port));
    });
  });
}

async function until(what: string, ok: () => Promise<boolean> | boolean, ms: number): Promise<void> {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    try {
      if (await ok()) return;
    } catch {}
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error(`never: ${what}`);
}

export async function hum(upstreamUrl: string, opts: { keys?: string[]; config?: Record<string, unknown>; hives?: string[] } = {}): Promise<Hum> {
  if (!existsSync(HUMD) || !existsSync(HUM)) throw new Error(`build hum and humd first: cargo build -p hum -p humd (looked in ${REPO}/target/debug)`);
  const root = mkdtempSync(join(tmpdir(), "oai-"));
  const env: Record<string, string> = {
    PATH: `${join(REPO, "target", "debug")}:${process.env.PATH ?? ""}`,
    HOME: root,
    XDG_STATE_HOME: join(root, "s"),
    XDG_CONFIG_HOME: join(root, "c"),
    XDG_DATA_HOME: join(root, "d"),
    XDG_CACHE_HOME: join(root, "k"),
    XDG_RUNTIME_DIR: join(root, "r"),
    HUM_THRUM_SOCK: join(root, "s", "hum", "thrum.sock"),
    HUM_CLI: HUM,
    HUM_LOG_LEVEL: process.env.E2E_LOG ?? "warn",
  };
  for (const d of ["s", "c/hum/hives.d", "c/hum/hives", "d", "k", "r"]) mkdirSync(join(root, d), { recursive: true });
  writeFileSync(join(root, "c/hum/hum.json"), JSON.stringify({ humd: { metricsAddr: "none", irohPort: 0 } }));
  for (const hive of ["openai-server", ...(opts.hives ?? [])]) writeFileSync(join(root, `c/hum/hives.d/${hive}.orch`), "# e2e\n");
  if (opts.config) writeFileSync(join(root, "c/hum/hives/openai-server.json"), JSON.stringify(opts.config));

  let log = "";
  const keep = (c: ChildProcess, who: string) => {
    c.stdout?.on("data", (b) => (log += `[${who}] ${b}`));
    c.stderr?.on("data", (b) => (log += `[${who}] ${b}`));
  };
  const children: ChildProcess[] = [];
  const humd = spawn(HUMD, [], { env, stdio: ["ignore", "pipe", "pipe"] });
  keep(humd, "humd");
  children.push(humd);
  await until("humd's thrum socket", () => existsSync(env.HUM_THRUM_SOCK), 30_000);

  const keys: Record<string, string> = {};
  for (const name of opts.keys ?? []) {
    const made = spawnSync("node", [join(HIVE_DIR, "dist", "index.js"), "key", "add", name], { env, encoding: "utf8" });
    if (made.status !== 0) throw new Error(`key add ${name}: ${made.stderr}`);
    keys[name] = made.stdout.trim();
  }

  const port = await freePort();
  const server = spawn("node", [join(HIVE_DIR, "dist", "index.js")], {
    env: { ...env, OPENAI_SERVER_PORT: String(port), OPENAI_API_BASE: upstreamUrl, OPENAI_API_KEY: "sk-upstream" },
    stdio: ["ignore", "pipe", "pipe"],
  });
  keep(server, "server");
  children.push(server);
  const lookup = spawn("bun", [join(HIVE_DIR, "test", "lookup-bee.ts")], { env, stdio: ["ignore", "pipe", "pipe"] });
  keep(lookup, "lookup");
  children.push(lookup);

  const base = `http://127.0.0.1:${port}/v1`;
  const bearer = Object.values(keys)[0];
  await until("the server lists the upstream's models and scents", async () => {
    const r = await fetch(`${base}/models`, { headers: bearer ? { Authorization: `Bearer ${bearer}` } : {} });
    const ids = ((await r.json()) as { data?: Array<{ id: string }> }).data?.map((m) => m.id) ?? [];
    return ids.includes("gpt-lab") && ids.includes("text-embedding-lab");
  }, 60_000).catch((e) => {
    throw new Error(`${e.message}\n${log}`);
  });
  await until("the lookup attendant nestles", () => log.includes("lookup nestled"), 30_000).catch((e) => {
    throw new Error(`${e.message}\n${log}`);
  });

  return {
    root,
    env,
    base,
    keys,
    log: () => log,
    close: async () => {
      for (const c of children.reverse()) {
        c.kill("SIGTERM");
        await new Promise((r) => (c.exitCode !== null ? r(null) : c.once("exit", r)));
      }
      rmSync(root, { recursive: true, force: true });
    },
  };
}

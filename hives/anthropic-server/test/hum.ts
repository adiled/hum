// A hum of its own for each test file: a humd from this checkout, the
// scripted nurse (nurse-bee.ts) and this hive's server, under Node as it
// ships, and nothing of the person's.
//
// Everything lives under one fresh directory, short enough for a unix
// socket: hum's config, state, data, cache and runtime roots, so the humd,
// every `hum bee link` and every key it enters stay there. The hives are
// installed by their markers: this one, and `lab`, the nurse's. Every child
// is given its environment explicitly, so none can reach a humd but this
// one.

import { spawn, type ChildProcess } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const HIVE_DIR = resolve(import.meta.dir, "..");
const REPO = resolve(HIVE_DIR, "../..");
const HUM = join(REPO, "target", "debug", "hum");
const HUMD = join(REPO, "target", "debug", "humd");

export interface Heard {
  prompts: any[];
  results: any[];
  cancels: string[];
}

export interface Hum {
  root: string;
  base: string;
  heard: () => Promise<Heard>;
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

async function until(what: string, ok: () => boolean, ms: number): Promise<void> {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    if (ok()) return;
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error(`never: ${what}`);
}

export async function hum(opts: { env?: Record<string, string>; config?: Record<string, unknown> } = {}): Promise<Hum> {
  if (!existsSync(HUMD) || !existsSync(HUM)) throw new Error(`build hum and humd first: cargo build -p hum -p humd (looked in ${REPO}/target/debug)`);
  const root = mkdtempSync(join(tmpdir(), "ant-"));
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
  for (const hive of ["anthropic-server", "lab"]) writeFileSync(join(root, `c/hum/hives.d/${hive}.orch`), "# e2e\n");
  if (opts.config) writeFileSync(join(root, "c/hum/hives/anthropic-server.json"), JSON.stringify(opts.config));

  let log = "";
  const keep = (c: ChildProcess, who: string) => {
    c.stdout?.on("data", (b) => (log += `[${who}] ${b}`));
    c.stderr?.on("data", (b) => (log += `[${who}] ${b}`));
  };
  const children: ChildProcess[] = [];
  const close = async () => {
    for (const c of children.reverse()) {
      c.kill("SIGTERM");
      await new Promise((r) => (c.exitCode !== null || c.signalCode !== null ? r(null) : c.once("exit", r)));
    }
    rmSync(root, { recursive: true, force: true });
  };
  const fail = async (e: Error) => {
    await close();
    throw new Error(`${e.message}\n${log}`);
  };

  const humd = spawn(HUMD, [], { env, stdio: ["ignore", "pipe", "pipe"] });
  keep(humd, "humd");
  children.push(humd);
  await until("humd's thrum socket", () => existsSync(env.HUM_THRUM_SOCK), 30_000).catch(fail);

  const nurse = spawn("bun", [join(HIVE_DIR, "test", "nurse-bee.ts")], { env, stdio: ["ignore", "pipe", "pipe"] });
  keep(nurse, "nurse");
  children.push(nurse);
  await until("the nurse nestles", () => /nurse nestled (\S+)/.test(log), 30_000).catch(fail);
  const nurseUrl = log.match(/nurse nestled (\S+)/)![1];

  const port = await freePort();
  const server = spawn("node", [join(HIVE_DIR, "dist", "index.js")], {
    env: { ...env, ANTHROPIC_SERVER_PORT: String(port), ...opts.env },
    stdio: ["ignore", "pipe", "pipe"],
  });
  keep(server, "server");
  children.push(server);
  await until("the server's bee nestles", () => log.includes("anthropic-server listening on"), 60_000).catch(fail);

  return {
    root,
    base: `http://127.0.0.1:${port}`,
    heard: async () => (await (await fetch(nurseUrl)).json()) as Heard,
    log: () => log,
    close,
  };
}

// A hum of its own for each test file: a humd from this checkout and the
// lab's bees (lab.ts), and nothing of the person's.
//
// Everything lives under one fresh directory, short enough for a unix
// socket: hum's config, state, data, cache and runtime roots, so the humd,
// every `hum bee link` and every key it enters stay there. The hives
// `vercel-ai` and `lab` are installed by their markers. The provider runs
// in the test's own process, so `use()` points this process's environment
// at the hum; every child is given its environment explicitly.

import { spawn, type ChildProcess } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import type { Tone } from "../../../thrum-clients/ts/index.ts";

const HIVE_DIR = resolve(import.meta.dir, "..");
const REPO = resolve(HIVE_DIR, "../..");
export const HUM = join(REPO, "target", "debug", "hum");
const HUMD = join(REPO, "target", "debug", "humd");

export interface Hum {
  root: string;
  env: Record<string, string>;
  heard: () => Tone[];
  bees: () => Record<string, { hive: string; role?: string; hid?: string; propensity?: unknown; chis?: string[]; source?: string }>;
  use: () => void;
  log: () => string;
  close: () => Promise<void>;
}

export async function until(what: string, ok: () => Promise<boolean> | boolean, ms: number): Promise<void> {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    try {
      if (await ok()) return;
    } catch {}
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error(`never: ${what}`);
}

export async function hum(): Promise<Hum> {
  if (!existsSync(HUMD) || !existsSync(HUM)) throw new Error(`build hum and humd first: cargo build -p hum -p humd (looked in ${REPO}/target/debug)`);
  const root = mkdtempSync(join(tmpdir(), "vai-"));
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
    LAB_LOG: join(root, "lab.jsonl"),
  };
  for (const d of ["s", "c/hum/hives.d", "c/hum/hives", "d", "k", "r"]) mkdirSync(join(root, d), { recursive: true });
  writeFileSync(join(root, "c/hum/hum.json"), JSON.stringify({ humd: { metricsAddr: "none", irohPort: 0 } }));
  for (const hive of ["vercel-ai", "lab"]) writeFileSync(join(root, `c/hum/hives.d/${hive}.orch`), "# e2e\n");
  writeFileSync(env.LAB_LOG, "");

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

  const lab = spawn("bun", [join(HIVE_DIR, "test", "lab.ts")], { env, stdio: ["ignore", "pipe", "pipe"] });
  keep(lab, "lab");
  children.push(lab);
  await until("the lab's bees nestle", () => log.includes("lab nestled"), 30_000).catch((e) => {
    throw new Error(`${e.message}\n${log}`);
  });

  const bees = join(env.XDG_STATE_HOME, "hum", "bees.json");

  return {
    root,
    env,
    heard: () => readFileSync(env.LAB_LOG, "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l)),
    bees: () => (existsSync(bees) ? JSON.parse(readFileSync(bees, "utf8")) : {}),
    use: () => Object.assign(process.env, env),
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

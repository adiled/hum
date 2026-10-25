// Where openai-server keeps what it is told and what it learns.
//
// The hive's config is hum's for every hive, `hives/openai-server.json`
// under hum's config root, and its state lives under hum's state root,
// `hives/openai-server`. An environment variable wins over the file.
//
//   host, port    where the HTTP surface listens (OPENAI_SERVER_HOST, OPENAI_SERVER_PORT; 127.0.0.1:14620)
//   apiKey        a key named `default` (OPENAI_SERVER_API_KEY), beside the keys in secrets/keys.json
//   cors          the origins a browser may call from, or "*"; none by default
//   timeoutMs     how long one request may take (OPENAI_SERVER_TIMEOUT_MS; 15 minutes)
//   rate          { capacity, refillPerSec } for each key; no limit when unset
//   scents        the upstream models the scent attendant distils by; by default those whose id says embed

import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

export const HIVE = "openai-server";

export interface Rate {
  capacity: number;
  refillPerSec: number;
}

export interface Config {
  host: string;
  port: number;
  apiKey: string;
  cors: string[];
  timeoutMs: number;
  rate?: Rate;
  scents?: string[];
}

function xdg(name: string, fallback: string): string {
  const set = process.env[name];
  return set && set.length > 0 ? set : join(homedir(), fallback);
}

export function configRoot(): string {
  return join(xdg("XDG_CONFIG_HOME", ".config"), "hum");
}

export function stateDir(): string {
  return join(xdg("XDG_STATE_HOME", ".local/state"), "hum", "hives", HIVE);
}

function file(): Record<string, unknown> {
  try {
    const parsed = JSON.parse(readFileSync(join(configRoot(), "hives", `${HIVE}.json`), "utf8"));
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch {
    return {};
  }
}

function num(v: unknown): number | undefined {
  const n = typeof v === "string" ? Number(v) : v;
  return typeof n === "number" && Number.isFinite(n) ? n : undefined;
}

function strings(v: unknown): string[] | undefined {
  if (typeof v === "string") return [v];
  return Array.isArray(v) ? v.filter((s): s is string => typeof s === "string") : undefined;
}

export function loadConfig(): Config {
  const f = file();
  const env = process.env;
  const rate = f.rate as { capacity?: unknown; refillPerSec?: unknown } | undefined;
  const capacity = num(rate?.capacity);
  const refillPerSec = num(rate?.refillPerSec);
  return {
    host: env.OPENAI_SERVER_HOST ?? (typeof f.host === "string" ? f.host : "127.0.0.1"),
    port: num(env.OPENAI_SERVER_PORT) ?? num(f.port) ?? 14620,
    apiKey: env.OPENAI_SERVER_API_KEY ?? (typeof f.apiKey === "string" ? f.apiKey : ""),
    cors: strings(f.cors) ?? [],
    timeoutMs: num(env.OPENAI_SERVER_TIMEOUT_MS) ?? num(f.timeoutMs) ?? 15 * 60_000,
    rate: capacity && refillPerSec ? { capacity, refillPerSec } : undefined,
    scents: strings(f.scents),
  };
}

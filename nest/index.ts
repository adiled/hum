import type { HumConfig } from "../fs/config.ts";
import { PipePerch } from "../nests/claude-cli/perch.ts";
import { PtyPerch } from "../nests/claude-repl/perch.ts";
import type { Perch } from "./perch.ts";

export type { RoostProc } from "./types.ts";
export type { Perch, PerchSpawnArgs } from "./perch.ts";
export { PipePerch } from "../nests/claude-cli/perch.ts";
export { PtyPerch } from "../nests/claude-repl/perch.ts";

// Daemon-side picker. Resolution order:
//   1. explicit — nestler dictated `nest` at handshake (highest priority)
//   2. cfg.nest — global default
export function pickPerch(cfg: HumConfig, _cwd?: string, explicit?: "claude-repl" | "claude-cli"): Perch {
  if (explicit === "claude-cli") return new PipePerch();
  if (explicit === "claude-repl") return new PtyPerch();
  return cfg.nest === "claude-cli" ? new PipePerch() : new PtyPerch();
}

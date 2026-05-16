import type { HumConfig } from "../fs/config.ts";
import { PipePerch } from "../nests/claude-cli/perch.ts";
import { PtyPerch } from "../nests/claude-repl/perch.ts";
import type { Perch } from "./perch.ts";

export type { RoostProc } from "./types.ts";
export type { Perch, PerchSpawnArgs } from "./perch.ts";
export { PipePerch } from "../nests/claude-cli/perch.ts";
export { PtyPerch } from "../nests/claude-repl/perch.ts";

// Daemon-side picker. Resolution order:
//   1. cfg.projects[cwd].nest — per-project override (when cwd matches)
//   2. cfg.nest               — global default
// Nest values:
//   "claude-repl" → PTY-driven Ink REPL, bills against Pro/Max subscription
//   "claude-cli"  → legacy `-p` headless pipe, bills against API credits
export function pickPerch(cfg: HumConfig, cwd?: string): Perch {
  let override: "claude-repl" | "claude-cli" | undefined;
  if (cwd && Array.isArray(cfg.projects)) {
    for (const entry of cfg.projects) {
      if (entry.primaryPath === cwd && entry.nest) { override = entry.nest; break; }
    }
  }
  const nest = override ?? cfg.nest;
  return nest === "claude-cli" ? new PipePerch() : new PtyPerch();
}

import type { RoostProc } from "./types.ts";

// Args passed to a perch's spawn() — the daemon assembles the
// command line + env once, then the perch decides how to bring up
// the Claude CLI process (piped child or PTY harness).
//
// "Perch" is how a roost is mounted. A roost (one Claude CLI process)
// can perch on a pipe (legacy `-p` mode) or on a PTY (subscription
// mode). The Perch interface abstracts that choice.
export interface PerchSpawnArgs {
  command: string;
  args: string[];
  cwd: string;
  env: Record<string, string>;
  /**
   * Hint from the daemon: when known, the Claude session id the
   * transcript will land at. PTY mode needs this so the harness can
   * poll the right JSONL file; pipe mode ignores it.
   */
  harnessSessionId?: string;
  /**
   * Absolute path to the JSONL transcript file. Only consumed by the
   * PTY perch. Pipe mode ignores it.
   */
  transcriptPath?: string;
  /**
   * Daemon-side hook called by the perch/harness on key lifecycle
   * events. Drift telemetry is plumbed through this; harness stays
   * decoupled from session-level state.
   */
  onPerfMark?: (event: string, span?: { name: string; ms: number }) => void;
}

export interface Perch {
  /** True for one-shot roosts that the daemon should evict on each `result`. */
  ephemeral: boolean;
  spawn(args: PerchSpawnArgs): RoostProc;
}

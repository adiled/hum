import { Readable } from "stream";
import type { RoostProc } from "../../nest/types.ts";
import type { Perch, PerchSpawnArgs } from "../../nest/perch.ts";
import { createHarness, promptTextFromJson } from "./harness.ts";

// PtyPerch — Claude CLI perched on an interactive PTY, with the harness
// composing pty/* (DEC responder + modal watcher), hooks/* (FIFO + relay
// script), and transcript/* (JSONL → stream-json synthesis).
//
// One spawn handles ONE turn — Ink doesn't reliably return to the input
// box after Stop, so the harness kills the PTY 50ms after emitting
// `result`. Daemon's pool sees `ephemeral: true` and evicts on each
// turn, re-spawning with --resume for the next.
export class PtyPerch implements Perch {
  readonly ephemeral = true;

  spawn(args: PerchSpawnArgs): RoostProc {
    if (!args.transcriptPath || !args.harnessSessionId) {
      throw new Error("PtyPerch.spawn requires transcriptPath and harnessSessionId");
    }
    const harness = createHarness(
      args.command,
      args.args,
      { cwd: args.cwd, env: args.env, onPerfMark: args.onPerfMark },
      args.transcriptPath,
      args.harnessSessionId,
    );
    return {
      pid: harness.pty.pid,
      stdin: {
        // PTY mode: Claude CLI runs interactive, MCP tool calls round-trip
        // through hum's MCP server natively. Only the typed user prompt
        // needs to be injected. Tool results flow back via MCP response,
        // not via stdin — the old `<tool_result>` XML write was a dead path.
        write: (data: string) => {
          const text = promptTextFromJson(data);
          if (text) harness.stdin.write(text);
          return true;
        },
      },
      stdout: harness.readable,
      stderr: new Readable({ read() {}, destroy(err, cb) { cb(err ?? null); } }),
      kill: () => { try { harness.pty.kill(); } catch {} finally { harness.cleanup(); } },
      exited: new Promise<number>(resolve => harness.pty.onExit(({ exitCode }) => resolve(exitCode ?? 1))),
    };
  }
}

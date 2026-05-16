import { spawn as nodeSpawn } from "node:child_process";
import type { RoostProc } from "../../nest/types.ts";
import type { Perch, PerchSpawnArgs } from "../../nest/perch.ts";

// PipePerch — Claude CLI perched on `-p` (print/pipe) mode. stdin/stdout/stderr
// are real pipes; the daemon talks to it via stream-json over stdio. The
// process is long-lived: one spawn handles many turns.
export class PipePerch implements Perch {
  readonly ephemeral = false;

  spawn(args: PerchSpawnArgs): RoostProc {
    const proc = nodeSpawn(args.command, args.args, {
      cwd: args.cwd,
      env: args.env,
      stdio: ["pipe", "pipe", "pipe"],
    });
    return {
      pid: proc.pid,
      stdin: proc.stdin!,
      stdout: proc.stdout!,
      stderr: proc.stderr!,
      kill: (signal?) => proc.kill(signal as any),
      exited: new Promise<number>(resolve => proc.on("exit", (code) => resolve(code ?? 1))),
    };
  }
}

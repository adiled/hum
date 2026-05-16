import { mkdirSync, writeFileSync, chmodSync, unlinkSync, rmdirSync } from "fs";
import { join } from "path";
import { tmpdir } from "os";
import { randomBytes } from "crypto";
import { execSync } from "child_process";
import { buildHookSettings } from "./settings.ts";

// ─── Hook plumbing — FIFO + relay script + --settings JSON ──────────────
//
// Claude Code exposes a hooks system via `--settings`. We register two:
// SessionStart fires once the Ink UI is mounted and ready to accept
// keystrokes; Stop fires when the assistant finishes a turn (with the
// transcript_path that holds the JSONL we need to replay). Both events
// arrive deterministically over a named pipe — no screen scraping, no
// version-dependent prose matching, no `╭`/`❯` heuristics.
//
// Inspired by smithersai/claude-p which proved the approach works
// against real Claude Code releases.

export interface HookHarness {
  tmpDir: string;
  fifoPath: string;
  scriptPath: string;
  settingsJson: string;
  cleanup: () => void;
}

const HOOK_RELAY_SH = `#!/bin/sh
# Relay a Claude Code hook event to hum's FIFO.
#   $1 = event name (e.g. "Stop", "SessionStart")
# stdin = the hook's JSON payload.
set -eu
event="$1"
fifo="\${HUM_HOOK_FIFO:?missing HUM_HOOK_FIFO}"
payload="$(cat | tr -d '\\n')"
printf '%s\\t%s\\n' "$event" "$payload" >> "$fifo"
exit 0
`;

export function createHookHarness(): HookHarness {
  const rand = randomBytes(4).toString("hex");
  const tmpDir = join(tmpdir(), `hum-hook-${process.pid}-${rand}`);
  mkdirSync(tmpDir, { recursive: true, mode: 0o700 });
  const fifoPath = join(tmpDir, "events.fifo");
  const scriptPath = join(tmpDir, "hook.sh");
  execSync(`mkfifo -m 0600 "${fifoPath}"`);
  writeFileSync(scriptPath, HOOK_RELAY_SH, { mode: 0o700 });
  chmodSync(scriptPath, 0o700);
  const settingsJson = buildHookSettings(scriptPath);
  return {
    tmpDir,
    fifoPath,
    scriptPath,
    settingsJson,
    cleanup: () => {
      try { unlinkSync(fifoPath); } catch {}
      try { unlinkSync(scriptPath); } catch {}
      try { rmdirSync(tmpDir); } catch {}
    },
  };
}

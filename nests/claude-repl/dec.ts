import { spawn as ptySpawn, type IPty } from "node-pty";

// ─── DEC / XTerm capability responder ───────────────────────────────────
//
// Ink, Claude CLI's TUI runtime, probes terminal capabilities at boot
// (DA1, DA2, DSR, XTVERSION, cursor-position, window-size). Real
// terminals reply within milliseconds. node-pty's PTY pair has nothing
// behind it that would answer, so we answer ourselves — without these
// replies Ink stalls forever and the UI never paints.

const ESC = "\x1b";

const DEC_RESPONSES = new Map<string, string>([
  [`${ESC}[c`, `${ESC}[?1;2c`],         // DA1
  [`${ESC}[>c`, `${ESC}[>0;0;0c`],      // DA2
  [`${ESC}[6n`, `${ESC}[1;1R`],         // DSR — cursor position
  [`${ESC}[>q`, `${ESC}P>|hum${ESC}\\`], // XTVERSION
  [`${ESC}[18t`, `${ESC}[8;40;120t`],   // window-size in chars
]);

function responderTick(decBuf: string): { response: string; remaining: string; consumed: boolean } {
  for (const [pat, resp] of DEC_RESPONSES) {
    const idx = decBuf.indexOf(pat);
    if (idx >= 0) {
      return { response: resp, remaining: decBuf.slice(0, idx) + decBuf.slice(idx + pat.length), consumed: true };
    }
  }
  return { response: "", remaining: decBuf, consumed: false };
}

export function spawnPty(command: string, args: string[], opts: { cwd: string; env: Record<string, string> }): IPty {
  const proc = ptySpawn(command, args, {
    name: "xterm-256color",
    cols: 120,
    rows: 40,
    cwd: opts.cwd,
    env: opts.env,
  });

  let decBuf = "";
  proc.onData((data: string) => {
    decBuf += data;
    if (decBuf.length > 131072) decBuf = decBuf.slice(-65536);
    while (true) {
      const { response, remaining, consumed } = responderTick(decBuf);
      if (response) {
        proc.write(response);
        decBuf = remaining;
        continue;
      }
      if (!consumed) break;
    }
  });

  return proc;
}

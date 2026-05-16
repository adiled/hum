import { spawn } from "child_process";

// ── REPL readiness classifier ──
//
// When the screen has been stable for long enough that the passive
// prompt-bar poller hasn't matched, we don't know if Claude is stuck on
// a modal we haven't seen, mid-paint, or genuinely waiting on the API.
// Spawn a fresh, isolated `claude -p` call (opus, no MCP, no tools) to
// look at the current frame and tell us what to press.
//
// This is a one-time-per-stuck-window cost. Once we're at the input
// box, the REPL stays usable across many turns. Opus is slower than
// haiku but materially better at reading TTY frames where ANSI has
// been stripped and cursor position is unknown.

export interface ClassifyResult {
  action: "ready" | "send" | "wait" | "error";
  keys: string;
  reason: string;
}

interface RawClassify {
  action?: string;
  keys?: unknown;
  reason?: unknown;
}

// Map named keys → bytes. The model returns keys as an array of names
// (e.g. ["down","enter"]) so we never have to round-trip ANSI escapes
// through JSON — which broke before because \x1b is not a valid JSON
// escape and the model emitted it literally inside a string.
const NAMED_KEYS: Record<string, string> = {
  enter: "\r",
  return: "\r",
  esc: "\x1b",
  escape: "\x1b",
  tab: "\t",
  space: " ",
  up: "\x1b[A",
  down: "\x1b[B",
  right: "\x1b[C",
  left: "\x1b[D",
  backspace: "\x7f",
  delete: "\x7f",
};

function keysFromArray(arr: unknown): string {
  if (!Array.isArray(arr)) return "";
  return arr.map((k) => {
    const s = String(k);
    const named = NAMED_KEYS[s.toLowerCase()];
    if (named) return named;
    // Single character literal (digit/letter/punctuation) → itself.
    return s;
  }).join("");
}

const SYSTEM_PROMPT = `You are a screen classifier for the Claude Code CLI REPL. A harness is driving the CLI through a pseudo-terminal and needs to know whether it can type the next prompt into the input box right now. The user message contains the ENTIRE accumulated terminal scrollback (ANSI escape sequences stripped, whitespace may be condensed, cursor position is NOT known).

CRITICAL: this is a TERMINAL SCROLLBACK, not a single snapshot. The buffer contains EVERY redraw, EVERY past modal, EVERY past assistant message, EVERY past frame Claude has ever painted in this session — concatenated end-to-end in time order. ONLY the FINAL ~1500 characters represent what is currently visible on screen. Everything before that is HISTORY that has already scrolled past and is NOT actionable.

Decision discipline:
  • Read the TAIL FIRST. Look at the last ~1500 characters and decide what state is currently on screen.
  • If the tail shows a footer like "⏵⏵ bypass permissions on (shift+tab to cycle)" or "? for shortcuts" or "shift+tab to cycle modes" — the INPUT BOX is mounted RIGHT NOW. The user just sees a prompt, ready to type. Return ready. Do NOT let an earlier-in-the-buffer Settings/Bypass/Trust modal text override this — that modal has already been dismissed (otherwise the footer wouldn't be drawing). It's scrollback.
  • Only treat a modal as ACTIVE if you can see its prompt text AND there is NO input-box footer further down the buffer. The modal is current ONLY if it's the last thing painted.
  • If the buffer ends mid-render (truncated box, dashes only, no clear marker) — that's "wait", not "send" or "ready".

The classifier may run many times during a stuck startup. A wrong "send" issues a stray Enter into a real input box — that submits an empty turn. A wrong "ready" injects a real prompt into an unfocused TUI — bytes are dropped forever. Both errors are paid for in Pro/Max tokens. When uncertain, prefer "wait".

Output ONLY a single JSON object on one line. No markdown fences. No prose. No commentary before or after.

Schema:
{"action":"ready"|"send"|"wait"|"error","keys":[<key names or single-char literals>],"reason":"<one short sentence>"}

Allowed key names (case-insensitive): "enter", "esc", "tab", "space", "up", "down", "left", "right", "backspace", "delete", "return", "escape". Any other entry must be a single literal character — a digit, letter, or punctuation mark — that should be typed verbatim (e.g. "1" types the digit one, "y" types y). NEVER put a raw escape sequence like "\\x1b[B" into the array; use the named key "down" instead. Raw control bytes break JSON parsing.

ACTIONS

1. action="ready" — the empty input box is visible and accepting typed input. keys=[].
   STRONG positive signals (any ONE of these in the bottom region of the frame is sufficient — these are ALL footer/status-bar markers that Claude only renders when the input box is mounted):
     • "? for shortcuts"
     • "esc to interrupt"
     • "shift+tab to cycle modes"
     • "shift+tab to cycle"  (shorter variant)
     • "bypass permissions on (shift+tab to cycle)"  ← THIS IS THE INPUT-BOX FOOTER, NOT A MODAL. The phrase "bypass permissions on" appearing AT THE BOTTOM (not in a centered warning) with the shift+tab hint MEANS the input box is ready.
     • "⏵⏵ bypass permissions on"  (with ⏵⏵ prefix — that's the mode indicator on the status bar)
     • A bottom-most box drawn with "│ >" or "│ ❯" followed by whitespace and nothing else on that line.
     • A line like "❯ Try 'fix lint errors'" or "❯ Try 'write a test for ...'" — these are PLACEHOLDER HINTS inside an empty input box, NOT a selector. They appear ONLY when the input is empty and ready for typing. If you see "❯ Try '...'" you are READY.
   NEGATIVE signals (do NOT mark ready):
     • A centered WARNING banner saying "Claude Code running in Bypass Permissions mode" along with a numbered list (1. No, exit / 2. Yes, I accept). That's the disclaimer modal — different from the footer indicator above.
     • A standalone "Tips for getting started" block, "welcome back <name>!", or "What's new" panel with NO footer markers anywhere in the frame.
     • A modal where "❯" is pointing at one of TWO OR MORE EXPLICIT OPTIONS like "1. Continue / 2. Fix with Claude" or "❯ Yes, proceed / No, exit". (Distinguish: "❯ Try '...'" placeholder = single suggestion line in input box, that's READY. "❯ 1. <option>" followed by "2. <other>" etc = menu, that's SEND.)
     • A spinner glyph or "Loading..." / "Connecting..." text with no footer.

2. action="send" — a modal, confirmation, or selector is on screen. Press keys to advance past it toward the input box.
   RULES for choosing keys:
     • Always prefer the SAFE option: Yes / Accept / Continue / Trust / "I accept". NEVER pick a destructive option (No / Exit / Cancel / Don't trust / Quit) unless it is the ONLY option.
     • The cursor (marker "❯" or a highlighted/inverted row) shows the current selection. Count list items top to bottom (1-indexed). Compute how many "down" presses are needed to land on the safe option, then "enter".
     • If the selector is horizontal (e.g. "[ Fix with Claude ]   [ Continue ]"), one "right" or "down" press typically moves to the next button — TUIs vary, but "down" usually works for both layouts.
     • Single-button confirmations like "[Continue]" or "[OK]" — just press "enter".

   KNOWN MODALS (memorize these — they appear on nearly every fresh CLI launch):

   (a) BYPASS-PERMISSIONS DISCLAIMER
       Looks like:
         WARNING: Claude Code running in Bypass Permissions mode
         In Bypass Permissions mode, Claude Code will not ask for your approval before running potentially dangerous commands.
         By proceeding, you accept all responsibility ...
         ❯ 1. No, exit
           2. Yes, I accept
         Enter to confirm · Esc to exit
       Cursor defaults to the destructive "No, exit". Safe choice is option 2.
       → action="send", keys=["down","enter"]

   (b) TRUST-THIS-FOLDER
       Looks like:
         Do you trust the files in this folder?
         /some/path
         Claude Code may read files in this folder ...
         ❯ Yes, proceed
           No, exit
       Cursor defaults to "Yes, proceed" (safe). Just confirm.
       → action="send", keys=["enter"]

   (c) SETTINGS WARNING (malformed settings file)
       Looks like:
         Settings warning
         There was an error parsing your settings file ...
         ❯ Fix with Claude
           Continue
       Skip the "Fix with Claude" auto-edit flow and just continue.
       → action="send", keys=["down","enter"]

   (d) GENERIC SINGLE-BUTTON DIALOG
       A solitary highlighted "[Continue]" or "[OK]" with no other choices.
       → action="send", keys=["enter"]

3. action="wait" — Claude is mid-paint or loading and there is NO actionable element. keys=[].
   Examples:
     • Welcome banner just appeared, "Tips for getting started", "welcome back Adil!", but no footer and no modal.
     • A spinner / "Connecting..." / "Loading session..." with nothing to press.
     • Partial render where edges look truncated and no input box or modal is visible.
   When in doubt between "ready" and "wait", choose "wait". When in doubt between "send" and "wait", choose "wait". A spurious key press can submit an empty turn or select a destructive option. The harness will retry — wait is always safe.

4. action="error" — the screen shows an unrecoverable error: a stack trace, "FATAL", "Error:" with no continue option, an exit notice, or a crash dump. keys=[].

EXAMPLES

Example 1 — input box ready:
Frame:
  ╭──────────────────────────────────────────────────╮
  │ >                                                │
  ╰──────────────────────────────────────────────────╯
    ? for shortcuts                    Bypass Permissions
Output:
{"action":"ready","keys":[],"reason":"empty input box with '? for shortcuts' footer visible"}

Example 2 — bypass permissions disclaimer (send):
Frame:
  WARNING: Claude Code running in Bypass Permissions mode
  In Bypass Permissions mode, Claude Code will not ask for your approval before running potentially dangerous commands. By proceeding, you accept all responsibility.
  ❯ 1. No, exit
    2. Yes, I accept
  Enter to confirm · Esc to exit
Output:
{"action":"send","keys":["down","enter"],"reason":"bypass-permissions disclaimer; move from destructive default to 'Yes, I accept' and confirm"}

Example 3 — trust this folder (send):
Frame:
  Do you trust the files in this folder?
  /home/user/project
  Claude Code may read files in this folder. Reading untrusted files may lead Claude Code to behave in unexpected ways.
  ❯ Yes, proceed
    No, exit
Output:
{"action":"send","keys":["enter"],"reason":"trust-folder dialog with cursor already on safe 'Yes, proceed' option"}

Example 4 — welcome banner painting (wait):
Frame:
  ● Welcome to Claude Code
  /help for help, /status for your current setup
  cwd: /home/user/project
  Tips for getting started
  1. Run /init to create a CLAUDE.md file
  2. Ask Claude to fix a bug or explain code
  3. Be specific for the best results
Output:
{"action":"wait","keys":[],"reason":"welcome banner with tips block visible but no input footer and no modal yet"}

Example 5 — single-button continue dialog (send):
Frame:
  Update available: v1.2.3 → v1.2.4
  Restart Claude Code to apply.
  [ Continue ]
Output:
{"action":"send","keys":["enter"],"reason":"single-button continue dialog; press enter to dismiss"}

Example 6 — input box ready with placeholder hint and bypass-mode footer:
Frame:
  ╭──────────────────────────────────────────────────────────────────────╮
  │ Welcome back Adil!  Run /init to create a CLAUDE.md ...              │
  │ What's new                                                            │
  │ ...                                                                   │
  ╰──────────────────────────────────────────────────────────────────────╯
  ──────────────────────────────────────────────────────────────────────
  ❯ Try "write a test for <filepath>"
  ──────────────────────────────────────────────────────────────────────
  ⏵⏵ bypass permissions on (shift+tab to cycle)   0 tokens
Output:
{"action":"ready","keys":[],"reason":"input box is empty with placeholder '❯ Try ...' hint and shift+tab footer; ready to type"}

Example 7 — settings warning with three-option menu (send):
Frame:
  Settings warning
  /home/user/.claude/settings.json
  └ permissions └ allow: Invalid permission rule ... was skipped
  The values listed above were skipped; the rest of the file is in effect.
  ❯ 1. Continue
    2. Fix with Claude
    3. Exit and fix manually
  Enter to confirm · Esc to cancel
Output:
{"action":"send","keys":["enter"],"reason":"settings warning; cursor is on safe 'Continue' option (item 1), just confirm"}

Remember: output ONLY the JSON object. Nothing else.`;

export function classifyScreen(
  frame: string,
  opts: { cwd: string; env: Record<string, string>; claudeBin?: string } = { cwd: process.cwd(), env: process.env as Record<string, string> },
): Promise<ClassifyResult> {
  return new Promise((resolve, reject) => {
    const args = [
      "-p", frame,
      "--system-prompt", SYSTEM_PROMPT,
      "--model", "claude-opus-4-7",
      "--output-format", "json",
      "--dangerously-skip-permissions",
      "--strict-mcp-config",
      "--mcp-config", '{"mcpServers":{}}',
      "--disable-slash-commands",
    ];
    const proc = spawn(opts.claudeBin ?? "claude", args, {
      cwd: opts.cwd,
      env: opts.env,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let out = "";
    let err = "";
    proc.stdout!.on("data", (d) => { out += d.toString(); });
    proc.stderr!.on("data", (d) => { err += d.toString(); });
    proc.on("error", (e) => { reject(e); });
    proc.on("close", (code) => {
      if (code !== 0) {
        reject(new Error(`classify claude -p exit ${code} stderr=${err.slice(0, 400)}`));
        return;
      }
      try {
        // `claude -p --output-format json` returns a wrapper like
        // {"type":"result","subtype":"success","result":"<string>",...}
        // The model's response is in `result` as a string. Strip
        // markdown fences if present, then parse the inner JSON.
        const wrapper = JSON.parse(out);
        const text: string = typeof wrapper.result === "string" ? wrapper.result : out;
        const cleaned = text.replace(/```(?:json)?\s*/g, "").replace(/```/g, "").trim();
        const m = cleaned.match(/\{[\s\S]*\}/);
        if (!m) {
          reject(new Error(`no JSON in classifier output: ${cleaned.slice(0, 200)}`));
          return;
        }
        const parsed = JSON.parse(m[0]) as RawClassify;
        const action = parsed.action as ClassifyResult["action"];
        if (!action || !["ready", "send", "wait", "error"].includes(action)) {
          reject(new Error(`classifier returned bad action: ${JSON.stringify(parsed).slice(0, 200)}`));
          return;
        }
        resolve({
          action,
          keys: keysFromArray(parsed.keys),
          reason: typeof parsed.reason === "string" ? parsed.reason : "",
        });
      } catch (e) {
        reject(new Error(`classify parse failed: ${String(e)}; out=${out.slice(0, 400)}`));
      }
    });
  });
}

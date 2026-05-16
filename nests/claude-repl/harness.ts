import { statSync } from "fs";
import { Readable } from "stream";
import type { IPty } from "node-pty";
import { trace } from "../../log.ts";

import { spawnPty } from "./dec.ts";
import { stripAnsi } from "./ansi.ts";
import { classifyScreen } from "./classify.ts";
import { createHookHarness } from "./hooks/fifo.ts";
import { injectSettingsArg } from "./hooks/settings.ts";
import { openFifoReader } from "./hooks/events.ts";
import { readTranscriptDelta, lastAssistant } from "./transcript/replay.ts";
import { synthesizeMessage, synthesizeResult } from "./transcript/synth.ts";
import type { AssistantMessage } from "./transcript/types.ts";

// ─── Harness state machine ──────────────────────────────────────────────
//
// Explicit transitions, single source of truth. Each transition logs
// via `trace("harness.state", { from, to, reason })`. Replaces the
// implicit cluster of claudeReady / pollerActive / sawTerminal booleans
// that used to drift apart.
//
// States borrow from the nest/bird domain (process lifecycle) — the
// plant cycle (bloom/petal/wilt/buds/shed/tendrils) is reserved for
// data/stream events the daemon emits during a turn. `WILTING` here
// is the one cross-over: it matches the existing `nest.wilt` trace
// fired at turn end, when the harness drains the transcript and
// emits the final petals.
//
//   NESTING  — spawn fired, building, pre-Ink (incl. modal dismissal)
//   PERCHED  — SessionStart fired, input box focused, addressable
//   HUNTING  — prompt injected, Claude off the perch generating
//   WILTING  — Stop fired, draining transcript, emitting petals
//   HUSHED   — drain finalized, kill scheduled (matches nest.hushed)
//   FELLED   — never reached PERCHED in 120s, or fatal error (matches nest.felled)

export type HarnessState =
  | "NESTING"
  | "PERCHED"
  | "HUNTING"
  | "WILTING"
  | "HUSHED"
  | "FELLED";

export interface HarnessHandle {
  pty: IPty;
  stdin: { write: (data: string) => void };
  readable: Readable;
  cleanup: () => void;
}

export function modelFromArgs(args: string[]): string {
  const idx = args.indexOf("--model");
  if (idx >= 0 && idx + 1 < args.length) return args[idx + 1];
  return "sonnet";
}

export function createHarness(
  command: string,
  args: string[],
  opts: {
    cwd: string;
    env: Record<string, string>;
    onPerfMark?: (event: string, span?: { name: string; ms: number }) => void;
  },
  transcriptPath: string,
  sessionId: string,
): HarnessHandle {
  const onPerfMark = opts.onPerfMark ?? (() => {});
  const hook = createHookHarness();
  const childEnv = { ...opts.env, HUM_HOOK_FIFO: hook.fifoPath };
  // Inject --settings just before the prompt (if any). Claude CLI parses
  // option flags up to the positional prompt arg.
  const augmentedArgs = injectSettingsArg(args, hook.settingsJson);
  trace("harness.hook.armed", { fifo: hook.fifoPath, script: hook.scriptPath });
  const spawnedAt = Date.now();
  const proc = spawnPty(command, augmentedArgs, { cwd: opts.cwd, env: childEnv });
  onPerfMark("repl_spawn");
  const modelId = modelFromArgs(args);

  // ── State ──
  let state: HarnessState = "NESTING";
  function transition(to: HarnessState, reason: string): void {
    if (state === to) return;
    trace("harness.state", { from: state, to, reason });
    state = to;
  }

  let destroyed = false;
  // Start at end of existing transcript. On --resume, the file already
  // contains the prior turn's content; without this, the live poller
  // re-emits all of it on every new harness, surfacing duplicate
  // assistant messages in OC.
  let transcriptOffset = (() => {
    try { return statSync(transcriptPath).size; } catch { return 0; }
  })();
  const promptQueue: string[] = [];

  // Readable stream that downstream consumes as if Claude were emitting
  // stream-json on its stdout. First event: a synthetic system/init so
  // the caller has a session_id immediately.
  const readable = new Readable({
    read() {},
    destroy(err, callback) {
      destroyed = true;
      fifoReader.close();
      hook.cleanup();
      callback(err ?? null);
    },
  });
  readable.push(JSON.stringify({
    type: "system",
    subtype: "init",
    session_id: sessionId,
    model: modelId,
    tools: [],
  }) + "\n");

  function emitTranscriptDelta(path: string = transcriptPath): boolean {
    try {
      const { messages, nextOffset } = readTranscriptDelta(path, transcriptOffset);
      if (nextOffset <= transcriptOffset) return false;
      transcriptOffset = nextOffset;
      for (const tm of messages) {
        for (const line of synthesizeMessage(tm)) {
          if (!destroyed) readable.push(line + "\n");
        }
      }
      return messages.length > 0;
    } catch (e) {
      trace("harness.transcript.read.failed", { err: String(e) });
      return false;
    }
  }

  // While the turn is in flight, poll the transcript every TRANSCRIPT_POLL_MS
  // and forward any new assistant messages immediately. Without this, OC
  // sees nothing until Stop fires — which can be many seconds on Opus
  // long-form reasoning and looks like a hang.
  //
  // The poller is gated on state — it skips while DRAINING because both
  // the poller (every 250ms) and drain (every 50ms) call
  // readTranscriptDelta + advance transcriptOffset. If a poller tick
  // lands in the same window as a drain tick, the same bytes can be
  // read twice (drain wins offset, poller emits stale messages — or
  // vice versa).
  const TRANSCRIPT_POLL_MS = 250;
  const livePoller = setInterval(() => {
    if (destroyed) return;
    if (state === "WILTING" || state === "HUSHED" || state === "FELLED") return;
    emitTranscriptDelta();
  }, TRANSCRIPT_POLL_MS);

  // Drive prompt submission. With hook-based readiness we know exactly
  // when Ink is mounted (SessionStart) and exactly when each turn ends
  // (Stop), so injection is a single write — no retries, no echo
  // confirmation, no busy-spin.
  function injectPrompt(text: string): void {
    if (destroyed) return;
    // Match smithersai/claude-p: text + \r in one tick. Inserting a
    // delay made Ink treat the second write as a separate keystroke
    // that didn't dismiss the input field.
    proc.write(text + "\r");
  }

  // Post-inject health check. If the classifier mis-detects ready (e.g.
  // on `--resume` while Ink is still painting restored history), the
  // prompt is written into an unfocused TUI and silently dropped — no
  // API call, no Stop hook, indicator hangs forever. Watch for either
  // a transcript delta OR a chunk of pty bytes within INJECT_HEALTH_MS
  // of writing the prompt; if neither lands, treat as failed and
  // re-classify. No retry cap — opus is trusted to converge; the loop
  // will keep going until something works or the proc dies.
  const INJECT_HEALTH_MS = 4000;
  let injectRetries = 0;
  let injectHealthTimer: ReturnType<typeof setTimeout> | null = null;
  let injectedAt = 0;
  let bytesAtInject = 0;
  let transcriptOffsetAtInject = 0;

  function clearInjectHealth(): void {
    if (injectHealthTimer !== null) {
      clearTimeout(injectHealthTimer);
      injectHealthTimer = null;
    }
  }

  function flushPromptQueue(): void {
    if (destroyed) return;
    if (state !== "PERCHED") return;
    const next = promptQueue.shift();
    if (!next) return;
    trace("harness.prompt.inject", { len: next.length });
    transition("HUNTING", "prompt-injected");
    injectedAt = Date.now();
    bytesAtInject = screenBuf.length;
    transcriptOffsetAtInject = transcriptOffset;
    onPerfMark("repl_prompt_inject");
    injectPrompt(next);
    // Push the prompt back to the front of the queue speculatively in
    // case the health check fires and re-classifies — saves the daemon
    // from re-sending. We pop it back off on success (first real Claude
    // activity).
    const pending = next;
    clearInjectHealth();
    injectHealthTimer = setTimeout(() => {
      injectHealthTimer = null;
      if (destroyed) return;
      if (state !== "HUNTING") return;
      const grewBytes = screenBuf.length - bytesAtInject;
      const grewTranscript = transcriptOffset - transcriptOffsetAtInject;
      // Transcript growth is the ONLY proof Claude processed the prompt.
      // PTY byte growth alone is noise — Ink redraws spinners and token
      // counters constantly even when the input was swallowed. A clean
      // inject must produce JSONL writes (assistant message blocks).
      if (grewTranscript > 0) {
        onPerfMark("repl_inject_healthy", { name: "repl_inject_to_first_byte", ms: Date.now() - injectedAt });
        trace("harness.inject.healthy", { grewBytes, grewTranscript, retries: injectRetries });
        return;
      }
      // No activity since inject. Either Ink wasn't focused or the
      // prompt was swallowed. Re-classify; no exhaustion limit, the
      // classifier owns convergence.
      injectRetries++;
      onPerfMark("repl_inject_unhealthy");
      trace("harness.inject.unhealthy.retry", {
        retries: injectRetries,
        idleMs: Date.now() - injectedAt,
        promptLen: pending.length,
      });
      // Restart the classify loop: requeue the prompt at head, drop
      // back to NESTING, reset attempts counter so classifier fires
      // again, and let the existing classifyTicker take over.
      promptQueue.unshift(pending);
      classifyAttempts = 0;
      lastChunkAt = Date.now();
      transition("NESTING", "inject-unhealthy");
    }, INJECT_HEALTH_MS);
  }

  function queuedWrite(data: string): void {
    trace("harness.write.enqueue", { state, len: data.length, sample: data.slice(0, 200) });
    promptQueue.push(data);
    if (state === "PERCHED") flushPromptQueue();
  }

  // ── PTY raw byte trace + screen snapshot ticker ──
  //
  // Two layers of observability that run for the *entire* proc lifetime
  // (until destroyed / HUSHED / FELLED), not just NESTING:
  //
  //   1. Per-chunk raw byte trace. Every onData chunk is logged with a
  //      length and a stripped-ANSI head — so we can see every Ink
  //      paint, modal redraw, or arrival of new content.
  //   2. tty.snap ticker every 2s — full cleaned tail of the screen
  //      buffer so we know WHAT is on screen at any moment.
  //
  // Previously both were NESTING-only, which left us blind exactly when
  // Claude wedged post-PERCHED (e.g. prompt injected but Ink not on
  // input box — Enter swallowed, no API call, no Stop hook).
  let screenBuf = "";
  let chunkSeq = 0;
  proc.onData((data: string) => {
    if (destroyed) return;
    screenBuf += data;
    if (screenBuf.length > 32768) screenBuf = screenBuf.slice(-16384);
    chunkSeq++;
    const head = stripAnsi(data).replace(/\s+/g, " ").slice(0, 200);
    trace("harness.pty.raw", { n: chunkSeq, state, len: data.length, head });
  });
  const ttyTicker = setInterval(() => {
    if (destroyed || state === "HUSHED" || state === "FELLED") {
      clearInterval(ttyTicker);
      return;
    }
    const clean = stripAnsi(screenBuf).slice(-1200);
    trace("harness.tty.snap", { state, len: screenBuf.length, tail: clean });
  }, 2000);

  // ── Readiness driver ──
  //
  // SessionStart hook is dropped — it fires before Ink commits to the
  // input-box state in some startup paths (welcome banner with home-dir
  // note, opus-4-5 cold start in large workspaces), so bytes injected
  // then were swallowed. The LLM classifier is the sole readiness
  // signal: when the screen has been stable for CLASSIFY_STABLE_MS,
  // spawn an isolated `claude -p` opus call with the full screenBuf;
  // it returns {action,keys,reason}. send → write keys; ready → PERCHED;
  // wait/error → loop. NO ATTEMPT CAP, NO TIMEOUT — opus is trusted to
  // converge. If it gets stuck, the system prompt is wrong; iterate on
  // the prompt rather than adding loop fences. Pauses while state isn't
  // NESTING so the inject health check can re-enter via NESTING.
  const CLASSIFY_STABLE_MS = 1200;
  let lastChunkAt = Date.now();
  let classifying = false;
  // classifyAttempts is kept purely for trace context (so we can see how
  // many rounds a stuck session needed). There is no cap — the classifier
  // is the sole authority on readiness and must be trusted to converge.
  // If it gets stuck, the system prompt is wrong, not the loop.
  let classifyAttempts = 0;
  // Hook the existing onData ticker so stability is measured from the
  // last byte we actually received (not wallclock since spawn).
  proc.onData(() => { lastChunkAt = Date.now(); });

  const classifyTicker = setInterval(async () => {
    if (destroyed) { clearInterval(classifyTicker); return; }
    // Pause-but-don't-clear when state isn't NESTING; the inject health
    // check may transition us BACK to NESTING after a failed injection
    // and we want this loop to resume firing — so just skip this tick.
    if (state !== "NESTING") return;
    if (classifying) return;
    if (Date.now() - lastChunkAt < CLASSIFY_STABLE_MS) return;
    if (screenBuf.length === 0) return;
    // Dump the entire visible scrollback to the classifier. Earlier
    // attempts to slice the tail (800, 1500) cut off footer markers
    // and forced opus to hallucinate from box-edge dashes. The point
    // of using an LLM here is precisely that it can read the whole
    // screen and figure out what's current — don't undermine it by
    // pre-trimming. screenBuf is already capped at 32K (lines above).
    const tail = stripAnsi(screenBuf);
    classifying = true;
    classifyAttempts++;
    const attempt = classifyAttempts;
    trace("harness.classify.start", { attempt, frameLen: tail.length, tailEnd: tail.slice(-300) });
    const classifyStartedAt = Date.now();
    onPerfMark("repl_classify_start");
    try {
      const result = await classifyScreen(tail, { cwd: opts.cwd, env: opts.env, claudeBin: command });
      onPerfMark("repl_classify_done", { name: "repl_classify", ms: Date.now() - classifyStartedAt });
      // result.keys contains literal control bytes (\r, \x1b[B …) which,
      // when console.log'd, get interpreted by journald as line breaks
      // and split a single trace into multiple entries — making the
      // trace effectively invisible to greps. Hex-encode for logging.
      const keysHex = Buffer.from(result.keys, "binary").toString("hex");
      trace("harness.classify.result", {
        attempt,
        action: result.action,
        keysHex,
        keysLen: result.keys.length,
        reason: result.reason,
      });
      if (state !== "NESTING") return;
      if (result.action === "ready") {
        // Do NOT clearInterval here — the inject health check can
        // transition back to NESTING if the prompt was swallowed, and
        // we need classifyTicker to fire again. The state-gate at the
        // top of the tick body pauses it while we're outside NESTING.
        clearTimeout(readyTimer);
        onPerfMark("repl_ready", { name: "repl_spawn_to_ready", ms: Date.now() - spawnedAt });
        transition("PERCHED", "ready:classify");
        trace("harness.ready", { via: "classify" });
        setTimeout(flushPromptQueue, 150);
      } else if (result.action === "send" && result.keys.length > 0) {
        proc.write(result.keys);
        // Reset stability window so the next tick waits for Claude's
        // response to our keypress before re-classifying.
        lastChunkAt = Date.now();
      }
      // wait / error: do nothing; ticker retries.
    } catch (e) {
      trace("harness.classify.failed", { attempt, err: String(e) });
    } finally {
      classifying = false;
    }
  }, 1000);

  // ── FIFO event reader ──
  const fifoReader = openFifoReader(hook.fifoPath, (event, payloadRaw) => handleHookEvent(event, payloadRaw));

  function handleHookEvent(event: string, payloadRaw: string): void {
    if (event === "Stop") {
      // Stop fires per assistant message. On `tool_use` the model is
      // pausing for an MCP/tool call — NOT finishing — so we keep
      // draining and only treat true end-of-turn reasons as terminal.
      // Terminal: "end_turn" | "max_tokens" | "stop_sequence".
      // Claude flushes the final JSONL line a few ms after Stop, often
      // in multiple writes — poll until we see a terminal stop_reason
      // or hit the wall cap (~3s).
      let payload: Record<string, unknown> = {};
      try { payload = JSON.parse(payloadRaw); } catch {}
      const txPath = (payload.transcript_path as string | undefined) || transcriptPath;
      const drainStartedAt = Date.now();
      transition("WILTING", "stop");
      onPerfMark("repl_drain_start");
      // Successful Stop means the inject worked — cancel the health timer.
      clearInjectHealth();
      injectRetries = 0;
      trace("harness.stop.drain.start", { txPath, offset: transcriptOffset });
      let attempts = 0;
      const MAX_ATTEMPTS = 60; // 60 × 50ms = 3s
      let sawTerminal = false;
      let lastMsg: AssistantMessage | null = null;
      const finalize = () => {
        if (destroyed) return;
        // If we drained zero messages, surface the failure to OC as
        // is_error rather than a phantom successful end_turn.
        const errored = lastMsg === null;
        readable.push(synthesizeResult(lastMsg, errored) + "\n");
        onPerfMark("repl_drain_finalized", { name: "repl_drain", ms: Date.now() - drainStartedAt });
        // Keep the REPL alive — Claude CLI returns to its input box
        // after the turn drains, so the same proc serves subsequent
        // prompts on this session. Transition straight back to PERCHED
        // and flush any queued prompt synchronously.
        trace("harness.turn.end.kept-alive", { sessionId, errored });
        onPerfMark("repl_turn_kept_alive");
        transition("PERCHED", "turn-finalized");
        flushPromptQueue();
      };
      const drain = () => {
        if (destroyed) return;
        if (sawTerminal) {
          finalize();
          return;
        }
        try {
          const { messages, nextOffset } = readTranscriptDelta(txPath, transcriptOffset);
          if (nextOffset > transcriptOffset) {
            transcriptOffset = nextOffset;
            for (const tm of messages) {
              for (const line of synthesizeMessage(tm)) {
                if (!destroyed) readable.push(line + "\n");
              }
              if (tm.kind === "assistant") {
                lastMsg = tm.msg;
                // Only true end-of-turn reasons terminate the drain.
                // `tool_use` means Claude is pausing mid-turn for a
                // tool call — keep draining.
                if (tm.msg.stop_reason === "end_turn" ||
                    tm.msg.stop_reason === "max_tokens" ||
                    tm.msg.stop_reason === "stop_sequence") {
                  sawTerminal = true;
                }
              }
            }
            if (messages.length > 0) {
              trace("harness.stop.drain.emitted", { messages: messages.length, offset: transcriptOffset, terminal: sawTerminal });
            }
          }
        } catch (e) {
          trace("harness.stop.drain.failed", { err: String(e) });
        }
        attempts++;
        if (!sawTerminal && attempts < MAX_ATTEMPTS) {
          setTimeout(drain, 50);
        } else {
          if (!sawTerminal) trace("harness.stop.drain.timeout", { attempts });
          finalize();
        }
      };
      drain();
      return;
    }
  }

  // No fallback ready-timer that *injects* the prompt. Writing before
  // Ink mounts feeds raw keystrokes to a non-input-box state and leaves
  // Claude wedged forever — worse than the wait. SessionStart is the
  // only valid "Ink is up, type now" signal.
  //
  // BUT if SessionStart never fires (broken hook script, missing
  // settings, killed pre-mount), OC sees no terminator and spins
  // forever. After 120s of no readiness, surface a synthetic error
  // result and gracefully end the readable stream.
  const readyTimer = setTimeout(() => {
    if (state === "PERCHED" || state === "HUNTING" || destroyed) return;
    transition("FELLED", "ready-timeout");
    trace("harness.ready.failed", { afterMs: 120_000 });
    try { readable.push(synthesizeResult(null, true) + "\n"); } catch {}
    try { readable.destroy(); } catch {}
  }, 120_000);

  return {
    pty: proc,
    stdin: { write: queuedWrite },
    readable,
    cleanup: () => {
      destroyed = true;
      clearTimeout(readyTimer);
      clearInterval(livePoller);
      clearInterval(ttyTicker);
      clearInterval(classifyTicker);
      clearInjectHealth();
      fifoReader.close();
      hook.cleanup();
      try { readable.destroy(); } catch {}
    },
  };
}

// ─── Daemon → harness stdin shim ────────────────────────────────────────
//
// Daemon writes Claude CLI user-message stream-json envelopes to
// harness.stdin.write. Hook mode doesn't accept stream-json — only
// typed text — so we extract the human prompt text for direct PTY
// injection. Tool results are not injected through the PTY (Claude CLI
// drives its own tool loop via the hook system).

export function promptTextFromJson(encodedJson: string): string {
  try {
    const parsed = JSON.parse(encodedJson);
    const content = parsed.message?.content;
    if (typeof content === "string") return content.trim() || "";
    if (Array.isArray(content)) {
      return content
        .filter((c: Record<string, unknown>) => c.type === "text")
        .map((c: Record<string, unknown>) => String(c.text ?? ""))
        .join("\n")
        .trim();
    }
    return "";
  } catch {
    return "";
  }
}

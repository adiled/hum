// One bloom, grown for one HTTP request.
//
// A request opens a bloom with a prompt, or carries its caller's answers
// to tools the bloom reached for, and then follows the bloom's sid. Its
// words and its thinking come as they unfurl. A tool the caller offered
// is the caller's to run only when humd reaches the caller with it,
// since humd may have handed the name to an attendant instead; the reach
// is held until the caller answers. Once reaches stop coming the turn goes
// back to the caller, finishing with its tool calls. A tool the server ran
// is told whole by `tendril-info`, never handed to the caller.
//
// The bloom finishes when it wilts, its reason said the OpenAI way, with
// the nectar it took. It fails when humd or the nurse says it broke off,
// with the status that fits: no nurse for the model is a 404, a prompt
// the nurse cannot grow is a 400, anything else from the nurse a 502. A
// bloom past its time is cancelled and fails as a 504, and one whose
// caller hung up is cancelled.

import type { Petal, Tone } from "../../../thrum-clients/ts/index.ts";
import { HttpError } from "./http.ts";
import type { Requester } from "./requester.ts";
import type { Answer } from "./conversation.ts";

const QUIET_MS = 150;

export type Finish = "stop" | "length" | "tool_calls" | "content_filter";

export interface Call {
  index: number;
  id: string;
  name: string;
  args: string;
}

export interface Served {
  callId: string;
  name: string;
  args: string;
  result: string;
  isError: boolean;
}

export interface Nectar {
  input_tokens?: number;
  output_tokens?: number;
  cache_read_input_tokens?: number;
  cache_creation_input_tokens?: number;
}

export interface Grown {
  finish: Finish;
  text: string;
  thought: string;
  calls: Call[];
  served: Served[];
  nectar?: Nectar;
}

export interface Listener {
  text?(delta: string): void;
  thought?(delta: string): void;
  call?(call: Call): void;
  served?(served: Served): void;
}

export interface Opening {
  prompt?: Tone<"prompt">;
  answers?: Answer[];
}

export function usageOf(nectar: Nectar | undefined): { prompt: number; completion: number; cached: number } {
  const cached = nectar?.cache_read_input_tokens ?? 0;
  const prompt = (nectar?.input_tokens ?? 0) + cached + (nectar?.cache_creation_input_tokens ?? 0);
  return { prompt, completion: nectar?.output_tokens ?? 0, cached };
}

export function finishOf(reason: string | undefined): Finish {
  switch (reason) {
    case "max_tokens":
    case "length":
      return "length";
    case "refusal":
    case "content_filter":
      return "content_filter";
    case "tool_use":
    case "tool_calls":
      return "tool_calls";
    default:
      return "stop";
  }
}

function json(v: unknown): string {
  if (typeof v === "string") return v;
  return v === undefined ? "{}" : JSON.stringify(v);
}

function failureOf(t: Tone<"error">): HttpError {
  const message = t.message || "the bloom broke off";
  switch (t.code) {
    case "no-nurse":
      return new HttpError(404, message, "model_not_found", "model");
    case "cannot-grow-on":
    case "empty-prompt":
      return new HttpError(400, message, t.code);
    case "humd-away":
      return new HttpError(503, message, t.code);
    default:
      return new HttpError(502, message, t.code ?? "nurse_error");
  }
}

export function grow(
  rq: Requester,
  sid: string,
  opening: Opening,
  opts: { timeoutMs: number; gone?: (fn: () => void) => void },
  on: Listener = {},
): Promise<Grown> {
  return new Promise<Grown>((resolve, reject) => {
    const grown: Grown = { finish: "stop", text: "", thought: "", calls: [], served: [] };
    const asked = new Set<string>();
    let quiet: NodeJS.Timeout | undefined;
    let settled = false;

    const settle = (rest: boolean, fn: () => void) => {
      if (settled) return;
      settled = true;
      clearTimeout(deadline);
      if (quiet) clearTimeout(quiet);
      rq.unfollow(sid, rest);
      fn();
    };
    const cancel = () => rq.send({ chi: "cancel", sid });

    const deadline = setTimeout(() => {
      cancel();
      settle(false, () => reject(new HttpError(504, `the bloom took longer than ${opts.timeoutMs} ms`, "timeout")));
    }, opts.timeoutMs);
    deadline.unref();

    opts.gone?.(() => {
      if (settled) return;
      cancel();
      settle(false, () => reject(new HttpError(499, "the caller went away", "client_closed")));
    });

    rq.follow(sid, (t: Tone) => {
      switch (t.chi) {
        case "unfurl": {
          if (t.closed || !t.text) return;
          if (t.kind === "text") {
            grown.text += t.text;
            on.text?.(t.text);
          } else if (t.kind === "sepal") {
            grown.thought += t.text;
            on.thought?.(t.text);
          }
          return;
        }
        case "tendril-reach": {
          if (asked.has(t.callId)) return;
          asked.add(t.callId);
          const call: Call = { index: grown.calls.length, id: t.callId, name: t.name, args: json(t.args) };
          grown.calls.push(call);
          rq.hold(sid, t.callId);
          on.call?.(call);
          if (quiet) clearTimeout(quiet);
          quiet = setTimeout(() => {
            grown.finish = "tool_calls";
            settle(true, () => resolve(grown));
          }, QUIET_MS);
          return;
        }
        case "tendril-info": {
          if (asked.has(t.callId) || (opening.answers ?? []).some((a) => a.callId === t.callId)) return;
          const served: Served = {
            callId: t.callId,
            name: t.name ?? "",
            args: json(t.args),
            result: typeof t.result === "string" ? t.result : json(t.result),
            isError: t.isError === true,
          };
          grown.served.push(served);
          on.served?.(served);
          return;
        }
        case "wilt":
          if (asked.size > 0) return;
          grown.finish = finishOf(t.reason);
          grown.nectar = (t.nectar ?? undefined) as Nectar | undefined;
          settle(false, () => resolve(grown));
          return;
        case "error":
          settle(false, () => reject(failureOf(t)));
          return;
      }
    });

    if (opening.answers && opening.answers.length > 0) {
      rq.release(opening.answers.map((a) => a.callId));
      for (const a of opening.answers) {
        rq.send({ chi: "tendril-result", sid, callId: a.callId, output: a.result, ...(a.isError ? { isError: true } : {}) });
      }
    } else if (opening.prompt) {
      rq.send(opening.prompt);
    }
  });
}

export function grewOf(grown: Grown): Petal[] {
  const petals: Petal[] = [];
  if (grown.text) petals.push({ kind: "text", text: grown.text });
  for (const c of grown.calls) petals.push({ kind: "tendril", callId: c.id, name: c.name, args: c.args });
  return petals;
}

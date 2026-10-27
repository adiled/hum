// One bloom, grown for one HTTP request.
//
// A request opens a bloom with a prompt, or carries its caller's results
// for tools the bloom reached for, and then follows the bloom's sid. Each
// petal is one content block, in the order it opened: a text petal a text
// block, a sepal a thinking block signed by its propolis. A tool the
// caller offered is the caller's to run only when humd reaches the caller
// with it, since humd may have handed the name to an attendant instead;
// each such reach is a tool_use block, held until the caller's tool_result
// comes. Once reaches stop coming the turn goes back to the caller, with
// `stop_reason: "tool_use"`.
//
// The bloom finishes when it wilts, its reason said the Anthropic way,
// with the nectar it took. It fails when humd or the nurse says it broke
// off, with the status that fits: no nurse for the model is a 404, a
// prompt the nurse cannot grow is a 400, humd away a 503, anything else
// from the nurse a 502. A bloom whose caller hung up is cancelled.

import { PetalKind, type Petal, type Tone } from "../../../thrum-clients/ts/index.ts";
import type { Answer } from "./conversation.ts";
import type { Forager } from "./forager.ts";
import { HttpError } from "./http.ts";

const QUIET_MS = 150;

export type Block =
  | { type: "text"; text: string }
  | { type: "thinking"; thinking: string; signature: string }
  | { type: "tool_use"; id: string; name: string; input: unknown };

export interface Usage {
  input_tokens: number;
  output_tokens: number;
  cache_creation_input_tokens?: number;
  cache_read_input_tokens?: number;
}

export interface Grown {
  content: Block[];
  stop: string;
  usage: Usage;
}

export interface Listener {
  start?(index: number, block: Block): void;
  delta?(index: number, delta: Record<string, unknown>): void;
  stop?(index: number): void;
}

export interface Opening {
  prompt?: Tone<"prompt">;
  answers?: Answer[];
}

export function stopReasonOf(reason: string | undefined): string {
  switch (reason) {
    case undefined:
    case "":
    case "stop":
    case "success":
      return "end_turn";
    case "length":
      return "max_tokens";
    default:
      return reason;
  }
}

export function usageOf(nectar: unknown): Usage {
  const n = (nectar && typeof nectar === "object" ? nectar : {}) as Record<string, unknown>;
  const num = (k: string) => (typeof n[k] === "number" ? (n[k] as number) : undefined);
  const usage: Usage = { input_tokens: num("input_tokens") ?? 0, output_tokens: num("output_tokens") ?? 0 };
  for (const k of ["cache_creation_input_tokens", "cache_read_input_tokens"] as const) {
    if (num(k) !== undefined) usage[k] = num(k);
  }
  return usage;
}

function inputOf(args: unknown): unknown {
  if (typeof args !== "string") return args ?? {};
  try {
    return JSON.parse(args);
  } catch {
    return {};
  }
}

function failureOf(t: Tone<"error">): HttpError {
  const message = t.message || "the bloom broke off";
  switch (t.code) {
    case "no-nurse":
      return new HttpError(404, message);
    case "cannot-grow-on":
    case "empty-prompt":
      return new HttpError(400, message);
    case "humd-away":
      return new HttpError(503, message);
    default:
      return new HttpError(502, message);
  }
}

export function grow(forager: Forager, sid: string, opening: Opening, gone: (fn: () => void) => void, on: Listener = {}): Promise<Grown> {
  return new Promise<Grown>((resolve, reject) => {
    const grown: Grown = { content: [], stop: "end_turn", usage: { input_tokens: 0, output_tokens: 0 } };
    const open = new Map<number, number>();
    const asked = new Set<string>();
    let quiet: NodeJS.Timeout | undefined;
    let settled = false;

    const settle = (rest: boolean, fn: () => void) => {
      if (settled) return;
      settled = true;
      if (quiet) clearTimeout(quiet);
      forager.unfollow(sid, rest);
      fn();
    };
    const close = (petal: number) => {
      const index = open.get(petal);
      if (index === undefined) return;
      open.delete(petal);
      on.stop?.(index);
    };
    const closeAll = () => {
      for (const petal of [...open.keys()]) close(petal);
    };

    gone(() => {
      if (settled) return;
      forager.send({ chi: "cancel", sid });
      settle(false, () => reject(new HttpError(499, "the caller went away")));
    });

    forager.follow(sid, (t: Tone) => {
      switch (t.chi) {
        case "unfurl": {
          if (t.closed) return close(t.petal);
          if (t.kind !== PetalKind.text && t.kind !== PetalKind.sepal) return;
          let index = open.get(t.petal);
          if (index === undefined) {
            index = grown.content.length;
            const block: Block = t.kind === PetalKind.text ? { type: "text", text: "" } : { type: "thinking", thinking: "", signature: "" };
            grown.content.push(block);
            open.set(t.petal, index);
            on.start?.(index, { ...block });
          }
          const block = grown.content[index];
          if (block.type === "text" && t.text) {
            block.text += t.text;
            on.delta?.(index, { type: "text_delta", text: t.text });
          } else if (block.type === "thinking") {
            if (t.text) {
              block.thinking += t.text;
              on.delta?.(index, { type: "thinking_delta", thinking: t.text });
            }
            if (t.propolis) {
              block.signature = t.propolis;
              on.delta?.(index, { type: "signature_delta", signature: t.propolis });
            }
          }
          return;
        }
        case "tendril-reach": {
          if (asked.has(t.callId)) return;
          asked.add(t.callId);
          closeAll();
          const index = grown.content.length;
          const input = inputOf(t.args);
          grown.content.push({ type: "tool_use", id: t.callId, name: t.name, input });
          forager.hold(sid, t.callId);
          on.start?.(index, { type: "tool_use", id: t.callId, name: t.name, input: {} });
          on.delta?.(index, { type: "input_json_delta", partial_json: JSON.stringify(input) });
          on.stop?.(index);
          if (quiet) clearTimeout(quiet);
          quiet = setTimeout(() => {
            grown.stop = "tool_use";
            settle(true, () => resolve(grown));
          }, QUIET_MS);
          return;
        }
        case "wilt":
          if (asked.size > 0) return;
          closeAll();
          grown.stop = stopReasonOf(t.reason);
          grown.usage = usageOf(t.nectar);
          settle(false, () => resolve(grown));
          return;
        case "error":
          settle(false, () => reject(failureOf(t)));
          return;
      }
    });

    if (opening.answers && opening.answers.length > 0) {
      forager.release(opening.answers.map((a) => a.callId));
      for (const a of opening.answers) {
        forager.send({ chi: "tendril-result", sid, callId: a.callId, output: a.result, ...(a.isError ? { isError: true } : {}) });
      }
    } else if (opening.prompt) {
      forager.send(opening.prompt);
    }
  });
}

export function grewOf(grown: Grown): Petal[] {
  return grown.content.flatMap<Petal>((b) => {
    if (b.type === "text") return b.text ? [{ kind: PetalKind.text, text: b.text }] : [];
    if (b.type === "thinking") return [{ kind: PetalKind.sepal, text: b.thinking, ...(b.signature ? { propolis: b.signature } : {}) }];
    return [{ kind: PetalKind.tendril, callId: b.id, name: b.name, args: b.input }];
  });
}

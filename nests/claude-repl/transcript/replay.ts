import { existsSync, openSync, readSync, statSync, closeSync } from "fs";
import type { AssistantBlock, AssistantMessage, ToolResultBlock, TranscriptMessage } from "./types.ts";

// ─── Transcript replay → message extraction ─────────────────────────────
//
// On every poll/drain, parse new lines from the JSONL transcript and
// pull out both:
//   1. Assistant messages (one JSONL line per content block — text /
//      thinking / tool_use), used by synth to emit content_block_* +
//      assistant envelope events.
//   2. User messages that carry tool_result blocks — Claude CLI writes
//      these as separate `user` lines after each tool_use to record
//      what the tool returned. Without surfacing these, OC sees the
//      tool_use without a matching tool_result and renders the call as
//      aborted (red). They're emitted as `type: "user"` envelopes that
//      the daemon's onMessage handler already knows how to map to
//      `petal("tool_result", ...)`.

export function readTranscriptDelta(path: string, fromOffset: number): { messages: TranscriptMessage[]; nextOffset: number } {
  if (!existsSync(path)) return { messages: [], nextOffset: fromOffset };
  const stat = statSync(path, { throwIfNoEntry: false });
  if (!stat || stat.size <= fromOffset) return { messages: [], nextOffset: fromOffset };
  const fd = openSync(path, "r");
  const buf = Buffer.alloc(stat.size - fromOffset);
  readSync(fd, buf, 0, buf.length, fromOffset);
  closeSync(fd);
  const msgs: TranscriptMessage[] = [];
  for (const line of buf.toString("utf8").split("\n").filter(Boolean)) {
    try {
      const parsed = JSON.parse(line);
      if (parsed.type === "assistant" && parsed.message?.role === "assistant") {
        const content: AssistantBlock[] = Array.isArray(parsed.message.content) ? parsed.message.content : [];
        msgs.push({
          kind: "assistant",
          msg: {
            uuid: parsed.uuid ?? "",
            content: content.map((c) => ({
              type: c.type as "text" | "tool_use" | "thinking",
              text: c.text as string | undefined,
              thinking: c.thinking as string | undefined,
              signature: c.signature as string | undefined,
              id: c.id as string | undefined,
              name: c.name as string | undefined,
              input: c.input as Record<string, unknown> | undefined,
            })),
            stop_reason: parsed.message.stop_reason as string | undefined,
            usage: parsed.message.usage as Record<string, number> | undefined,
            session_id: parsed.sessionId as string | undefined,
          },
        });
        continue;
      }
      if (parsed.type === "user" && parsed.message?.role === "user") {
        const raw = parsed.message.content;
        if (!Array.isArray(raw)) continue;
        const results: ToolResultBlock[] = [];
        for (const b of raw) {
          if (!b || typeof b !== "object") continue;
          if (b.type !== "tool_result") continue;
          if (typeof b.tool_use_id !== "string") continue;
          const body = b.content;
          let text = "";
          if (typeof body === "string") text = body;
          else if (Array.isArray(body)) {
            text = body
              .filter((c: Record<string, unknown>) => typeof c?.text === "string")
              .map((c: Record<string, unknown>) => c.text as string)
              .join("\n");
          }
          results.push({
            type: "tool_result",
            tool_use_id: b.tool_use_id,
            content: text,
            is_error: typeof b.is_error === "boolean" ? b.is_error : undefined,
          });
        }
        if (results.length > 0) {
          msgs.push({ kind: "user", msg: { uuid: parsed.uuid ?? "", tool_results: results } });
        }
        continue;
      }
    } catch { /* skip malformed */ }
  }
  return { messages: msgs, nextOffset: stat.size };
}

// Back-compat: extract just the latest assistant message in a list, so
// the Stop drain can keep tracking lastMsg (for stop_reason / usage).
export function lastAssistant(msgs: TranscriptMessage[]): AssistantMessage | null {
  for (let i = msgs.length - 1; i >= 0; i--) {
    if (msgs[i].kind === "assistant") return (msgs[i] as { kind: "assistant"; msg: AssistantMessage }).msg;
  }
  return null;
}

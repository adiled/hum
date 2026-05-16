import type { AssistantMessage, ToolResultBlock, TranscriptMessage, UserMessage } from "./types.ts";

// ─── Transcript replay → stream-json synthesis ──────────────────────────
//
// Turn each transcript message into the stream-json shape downstream
// daemon code understands:
//
//   - assistant messages → content_block_start/delta/stop per block,
//     followed by an `assistant` envelope. Daemon's onMessage maps the
//     envelope's tool_use blocks into petal("tool_call", ...) events.
//
//   - user messages with tool_result blocks → a `user` envelope with
//     each tool_result block intact. Daemon's onMessage maps these
//     into petal("tool_result", ...) events. Without this, OC sees
//     tool_use without a matching tool_result and the call is rendered
//     as aborted (red).

export function synthesizeMessage(tm: TranscriptMessage): string[] {
  if (tm.kind === "assistant") return synthesizeAssistant(tm.msg);
  return synthesizeUser(tm.msg);
}

function synthesizeAssistant(msg: AssistantMessage): string[] {
  const lines: string[] = [];
  if (!msg.content?.length) return lines;

  for (let i = 0; i < msg.content.length; i++) {
    const block = msg.content[i];
    const idx = i;
    if (block.type === "text" && block.text) {
      lines.push(JSON.stringify({ type: "content_block_start", index: idx, content_block: { type: "text", text: "" } }));
      lines.push(JSON.stringify({ type: "content_block_delta", index: idx, delta: { type: "text_delta", text: block.text } }));
      lines.push(JSON.stringify({ type: "content_block_stop", index: idx }));
    } else if (block.type === "thinking" && block.thinking) {
      lines.push(JSON.stringify({ type: "content_block_start", index: idx, content_block: { type: "thinking", thinking: "" } }));
      lines.push(JSON.stringify({ type: "content_block_delta", index: idx, delta: { type: "thinking_delta", thinking: block.thinking } }));
      lines.push(JSON.stringify({ type: "content_block_stop", index: idx }));
    } else if (block.type === "tool_use" && block.id && block.name) {
      lines.push(JSON.stringify({ type: "content_block_start", index: idx, content_block: { type: "tool_use", id: block.id, name: block.name, input: {} } }));
      if (block.input) {
        lines.push(JSON.stringify({ type: "content_block_delta", index: idx, delta: { type: "input_json_delta", partial_json: JSON.stringify(block.input) } }));
      }
      lines.push(JSON.stringify({ type: "content_block_stop", index: idx }));
    }
  }

  const synthContent: Record<string, unknown>[] = [];
  for (const block of msg.content) {
    if (block.type === "text" && block.text) {
      synthContent.push({ type: "text", text: block.text });
    } else if (block.type === "tool_use" && block.id && block.name) {
      synthContent.push({ type: "tool_use", id: block.id, name: block.name, input: block.input ?? {} });
    }
  }
  if (synthContent.length > 0) {
    lines.push(JSON.stringify({ type: "assistant", message: { content: synthContent } }));
  }
  return lines;
}

function synthesizeUser(msg: UserMessage): string[] {
  if (!msg.tool_results.length) return [];
  // Emit a single `user` envelope with all tool_result blocks. Daemon's
  // onMessage iterates content and emits petal("tool_result", ...) per
  // entry, which OC matches back to its open tool_use by tool_use_id.
  return [JSON.stringify({
    type: "user",
    message: {
      role: "user",
      content: msg.tool_results.map((r: ToolResultBlock) => ({
        type: "tool_result",
        tool_use_id: r.tool_use_id,
        content: r.content,
        ...(r.is_error !== undefined ? { is_error: r.is_error } : {}),
      })),
    },
  })];
}

// Emit a single `result` for a turn — daemon reads `result` as the
// turn-finished marker and wilts the roost listener for the next round.
// `isError` is set when the caller couldn't drain a real terminal
// message (drain timeout, ready timeout) — without it OC sees an
// empty "successful" turn.
export function synthesizeResult(msg: AssistantMessage | null, isError: boolean = false): string {
  const stopReason = msg?.stop_reason === "tool_use" ? "tool_use" : (msg?.stop_reason ?? "end_turn");
  return JSON.stringify({
    type: "result",
    stop_reason: stopReason,
    usage: msg?.usage ?? {},
    session_id: msg?.session_id ?? "",
    is_error: isError || msg === null,
  });
}

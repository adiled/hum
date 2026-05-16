// ─── Transcript types ───────────────────────────────────────────────────
//
// Shapes shared between the JSONL replayer and the stream-json synth.

export interface AssistantBlock {
  type: "text" | "tool_use" | "thinking";
  text?: string;
  thinking?: string;
  signature?: string;
  id?: string;
  name?: string;
  input?: Record<string, unknown>;
}

export interface AssistantMessage {
  uuid: string;
  content: AssistantBlock[];
  stop_reason?: string;
  usage?: Record<string, number>;
  session_id?: string;
}

// A user-side tool_result block. Claude CLI writes one of these per
// completed tool call into the JSONL transcript, as a separate `user`
// line after the assistant's tool_use line. Without surfacing these
// downstream, OC sees tool_use without matching tool_result and
// renders the tool call as aborted (red).
export interface ToolResultBlock {
  type: "tool_result";
  tool_use_id: string;
  content: string;
  is_error?: boolean;
}

export interface UserMessage {
  uuid: string;
  tool_results: ToolResultBlock[];
}

export type TranscriptMessage =
  | { kind: "assistant"; msg: AssistantMessage }
  | { kind: "user"; msg: UserMessage };

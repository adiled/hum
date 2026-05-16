export function encodePrompt(content: Array<Record<string, unknown>> | string): string {
  const parts = typeof content === "string"
    ? [{ type: "text", text: content }]
    : content;
  return JSON.stringify({
    type: "user",
    message: { role: "user", content: parts },
  });
}

export function encodeToolResult(toolUseId: string, result: string): string {
  return JSON.stringify({
    type: "user",
    message: { role: "user", content: [{ type: "tool_result", tool_use_id: toolUseId, content: result }] },
  });
}

export function parseLine(line: string): unknown {
  try { return JSON.parse(line); } catch { return null; }
}

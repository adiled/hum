// OpenAI-function → thrum-tool mapping.
//
// MCP wire-standard schema field is `inputSchema`; humd forwards
// this verbatim into the worker bee's MCP catalogue. Claude's mcp
// client zod-validates and rejects the entire `tools/list`
// response if any entry has `inputSchema: null` — so we MUST
// rename `parameters` and MUST emit at least `{}` when caller
// omits the schema.

export interface OpenAIFunction {
  name: string;
  description?: string;
  parameters?: Record<string, unknown>;
}
export interface OpenAITool {
  type?: "function";
  function?: OpenAIFunction;
}
export interface ToolSpec {
  name: string;
  description?: string;
  inputSchema: Record<string, unknown>;
}

export function toolsFromOpenAI(tools: OpenAITool[] | undefined): ToolSpec[] | undefined {
  if (!Array.isArray(tools) || tools.length === 0) return undefined;
  const out: ToolSpec[] = [];
  for (const t of tools) {
    if (t?.type !== "function" || !t.function?.name) continue;
    out.push({
      name: t.function.name,
      ...(t.function.description ? { description: t.function.description } : {}),
      inputSchema: t.function.parameters ?? {},
    });
  }
  return out.length > 0 ? out : undefined;
}

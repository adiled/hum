// A call's tools, as the tools a bloom may reach back to it for.
//
// A function tool becomes `{ name, description, inputSchema }` on the
// thrum, its name exactly as the SDK gave it. A schema is always an
// object, even when the tool gave none, since a cell's MCP client turns
// down a whole tool list over one missing schema. A provider tool is one
// only its own provider could run, and is left out. The tool choice says
// how the bloom's tendrils reach: auto, none, required, or a tool's name.

import type { LanguageModelV3CallOptions, LanguageModelV3ToolChoice } from "@ai-sdk/provider";

export interface ToolSpec {
  name: string;
  description?: string;
  inputSchema: Record<string, unknown>;
}

export function toolsOf(tools: LanguageModelV3CallOptions["tools"]): ToolSpec[] | undefined {
  const out: ToolSpec[] = [];
  for (const t of tools ?? []) {
    if (t.type !== "function" || !t.name) continue;
    const schema = t.inputSchema && typeof t.inputSchema === "object" ? (t.inputSchema as Record<string, unknown>) : { type: "object", properties: {} };
    out.push({ name: t.name, ...(t.description ? { description: t.description } : {}), inputSchema: schema });
  }
  return out.length > 0 ? out : undefined;
}

export function reachOf(choice: LanguageModelV3ToolChoice | undefined): string | undefined {
  if (!choice) return undefined;
  return choice.type === "tool" ? choice.toolName : choice.type;
}

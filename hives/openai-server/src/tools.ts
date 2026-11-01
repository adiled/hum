// A caller's tools, as the tools a bloom may reach back to it for.
//
// OpenAI says a function tool two ways: chat completions nest it,
// `{ type: "function", function: { name, description, parameters } }`,
// and the Responses API lays it flat, `{ type: "function", name,
// description, parameters }`. Either becomes `{ name, description,
// inputSchema }` on the thrum, its name exactly as the caller gave it. A
// schema is always an object, even when the caller gave none, since a
// cell's MCP client turns down a whole tool list over one missing schema.
// A tool of any other type is one only OpenAI could run, and is left out.

export interface OpenAITool {
  type?: string;
  name?: string;
  description?: string;
  parameters?: Record<string, unknown>;
  function?: { name?: string; description?: string; parameters?: Record<string, unknown> };
}

export interface ToolSpec {
  name: string;
  description?: string;
  inputSchema: Record<string, unknown>;
}

export function toolsFromOpenAI(tools: unknown): ToolSpec[] | undefined {
  if (!Array.isArray(tools) || tools.length === 0) return undefined;
  const out: ToolSpec[] = [];
  for (const t of tools as OpenAITool[]) {
    if (t?.type !== undefined && t.type !== "function") continue;
    const f = t?.function ?? t;
    if (!f?.name) continue;
    const schema = f.parameters && typeof f.parameters === "object" ? f.parameters : { type: "object", properties: {} };
    out.push({ name: f.name, ...(f.description ? { description: f.description } : {}), inputSchema: schema });
  }
  return out.length > 0 ? out : undefined;
}

export function reachOf(choice: unknown): string | undefined {
  if (typeof choice === "string") return choice;
  if (!choice || typeof choice !== "object") return undefined;
  const c = choice as { type?: string; name?: string; function?: { name?: string } };
  return c.function?.name ?? c.name ?? (c.type === "function" ? undefined : c.type);
}

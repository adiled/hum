// A caller's tools, as the tools a bloom may reach back to it for.
//
// A custom tool, `{ name, description, input_schema }`, becomes `{ name,
// description, inputSchema }` on the thrum, its name exactly as the caller
// gave it, its schema an object even when the caller gave none. A tool of
// any other type, web search, code execution, bash, a text editor, is one
// whose schema only Anthropic knows, and is left out.
//
// `tool_choice` is how the bloom reaches its tendrils: `auto` and `none`
// as they are, `any` as `required`, a named tool as its name, and
// `disable_parallel_tool_use` as not several at once.

import type { Tendrils } from "../../../thrum-clients/ts/index.ts";

export interface ToolSpec {
  name: string;
  description?: string;
  inputSchema: Record<string, unknown>;
}

export function toolsFromAnthropic(tools: unknown): ToolSpec[] | undefined {
  if (!Array.isArray(tools)) return undefined;
  const out: ToolSpec[] = [];
  for (const t of tools as Array<Record<string, any>>) {
    if (!t?.name || (t.type !== undefined && t.type !== null && t.type !== "custom")) continue;
    const schema = t.input_schema && typeof t.input_schema === "object" ? t.input_schema : { type: "object", properties: {} };
    out.push({ name: t.name, ...(t.description ? { description: t.description } : {}), inputSchema: schema });
  }
  return out.length > 0 ? out : undefined;
}

export function tendrilsOf(choice: unknown): Tendrils | undefined {
  if (!choice || typeof choice !== "object") return undefined;
  const c = choice as { type?: string; name?: string; disable_parallel_tool_use?: boolean };
  const reach = c.type === "any" ? "required" : c.type === "tool" ? c.name : c.type;
  const out: Tendrils = {
    ...(reach ? { reach } : {}),
    ...(c.disable_parallel_tool_use === true ? { parallel: false } : {}),
  };
  return Object.keys(out).length > 0 ? out : undefined;
}

// A caller's tools as thrum tools: the name exactly as given, the schema
// always an object, and a tool only Anthropic knows left out; and its
// tool_choice as how the bloom reaches.

import { describe, expect, test } from "bun:test";
import { tendrilsOf, toolsFromAnthropic } from "../src/tools.ts";

describe("toolsFromAnthropic", () => {
  test("a custom tool keeps its name, description and schema", () => {
    const schema = { type: "object", properties: { path: { type: "string" } } };
    expect(toolsFromAnthropic([{ name: "readFile", description: "Read", input_schema: schema }])).toEqual([{ name: "readFile", description: "Read", inputSchema: schema }]);
    expect(toolsFromAnthropic([{ type: "custom", name: "ping" }])).toEqual([{ name: "ping", inputSchema: { type: "object", properties: {} } }]);
  });

  test("a tool only Anthropic knows is left out, and no tools is none", () => {
    expect(toolsFromAnthropic([{ type: "web_search_20260209", name: "web_search" }, { type: "bash_20250124", name: "bash" }, { name: "x", input_schema: {} }])?.map((t) => t.name)).toEqual(["x"]);
    expect(toolsFromAnthropic([])).toBeUndefined();
    expect(toolsFromAnthropic(undefined)).toBeUndefined();
  });
});

describe("tendrilsOf", () => {
  test("a tool choice names what may reach, and whether several at once", () => {
    expect(tendrilsOf({ type: "auto" })).toEqual({ reach: "auto" });
    expect(tendrilsOf({ type: "any", disable_parallel_tool_use: true })).toEqual({ reach: "required", parallel: false });
    expect(tendrilsOf({ type: "tool", name: "readFile" })).toEqual({ reach: "readFile" });
    expect(tendrilsOf({ type: "none" })).toEqual({ reach: "none" });
    expect(tendrilsOf(undefined)).toBeUndefined();
  });
});

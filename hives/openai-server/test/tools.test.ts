// A caller's tools, in either of OpenAI's shapes, as thrum tools: the
// name exactly as given, the schema always an object, and a tool only
// OpenAI could run left out.

import { describe, expect, test } from "bun:test";
import { reachOf, toolsFromOpenAI } from "../src/tools.ts";

describe("toolsFromOpenAI", () => {
  test("chat's nested shape and the Responses API's flat one read alike", () => {
    const schema = { type: "object", properties: { path: { type: "string" } } };
    const nested = toolsFromOpenAI([{ type: "function", function: { name: "readFile", description: "Read", parameters: schema } }]);
    const flat = toolsFromOpenAI([{ type: "function", name: "readFile", description: "Read", parameters: schema }]);
    expect(nested).toEqual([{ name: "readFile", description: "Read", inputSchema: schema }]);
    expect(flat).toEqual(nested);
  });

  test("a tool with no schema gets an empty object", () => {
    expect(toolsFromOpenAI([{ type: "function", function: { name: "ping" } }])).toEqual([{ name: "ping", inputSchema: { type: "object", properties: {} } }]);
  });

  test("a tool only OpenAI runs is left out, and no tools is none", () => {
    expect(toolsFromOpenAI([{ type: "web_search" }, { type: "function", name: "x" }])?.map((t) => t.name)).toEqual(["x"]);
    expect(toolsFromOpenAI([])).toBeUndefined();
    expect(toolsFromOpenAI(undefined)).toBeUndefined();
  });

  test("a tool choice names what may reach", () => {
    expect(reachOf("required")).toBe("required");
    expect(reachOf({ type: "function", function: { name: "readFile" } })).toBe("readFile");
    expect(reachOf({ type: "function", name: "readFile" })).toBe("readFile");
    expect(reachOf(undefined)).toBeUndefined();
  });
});

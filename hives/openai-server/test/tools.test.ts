// Pins the OpenAI-tool → thrum-tool mapping. The wire field MUST
// be `inputSchema` — humd merges these into the worker's MCP
// catalogue and claude's mcp client rejects the entire
// `tools/list` response if any entry has `inputSchema: null`. A
// past regression named the field `parameters`, which served
// claude null schemas and made it claim it had no read/write
// tools.

import { describe, expect, test } from "bun:test";
import { toolsFromOpenAI } from "../src/tools.ts";

describe("toolsFromOpenAI", () => {
  test("renames `parameters` to `inputSchema` on the wire", () => {
    const out = toolsFromOpenAI([
      {
        type: "function",
        function: {
          name: "read",
          description: "Read a file",
          parameters: { type: "object", properties: { path: { type: "string" } } },
        },
      } as any,
    ])!;
    expect(out).toHaveLength(1);
    expect(out[0]).toEqual({
      name: "read",
      description: "Read a file",
      inputSchema: { type: "object", properties: { path: { type: "string" } } },
    });
    expect((out[0] as any).parameters).toBeUndefined();
  });

  test("emits empty-object schema when caller omits parameters", () => {
    const out = toolsFromOpenAI([
      { type: "function", function: { name: "ping" } } as any,
    ])!;
    expect(out[0].inputSchema).toEqual({});
    // Belt-and-suspenders: never null. claude-cli's mcp client
    // zod-validates inputSchema and rejects null.
    expect(out[0].inputSchema).not.toBeNull();
  });

  test("drops entries missing function name", () => {
    const out = toolsFromOpenAI([
      { type: "function" } as any,
      { type: "function", function: { name: "" } } as any,
      { type: "function", function: { name: "ok" } } as any,
    ]);
    expect(out).toHaveLength(1);
    expect(out![0].name).toBe("ok");
  });

  test("returns undefined for empty / missing input", () => {
    expect(toolsFromOpenAI(undefined)).toBeUndefined();
    expect(toolsFromOpenAI([])).toBeUndefined();
  });
});

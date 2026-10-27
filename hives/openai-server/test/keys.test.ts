// Keys: the file's, and the config's apiKey, which is never one of the
// file's by name.

import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { addKey, Keys, listKeys, removeKey } from "../src/keys.ts";

let was: string | undefined;

beforeAll(() => {
  was = process.env.XDG_STATE_HOME;
});

beforeEach(() => {
  process.env.XDG_STATE_HOME = mkdtempSync(join(tmpdir(), "oai-keys-"));
});

afterEach(() => {
  rmSync(process.env.XDG_STATE_HOME!, { recursive: true, force: true });
});

afterAll(() => {
  if (was === undefined) delete process.env.XDG_STATE_HOME;
  else process.env.XDG_STATE_HOME = was;
});

describe("keys", () => {
  test("no file key takes the config key's name", () => {
    expect(() => addKey("default")).toThrow();
    expect(() => addKey("anonymous")).toThrow();
    expect(listKeys().map((k) => k.name)).not.toContain("default");
  });

  test("the config key and every file key are each their own", () => {
    const alice = addKey("alice");
    const keys = new Keys("sk-configured");
    expect(keys.nameOf("sk-configured")).toBe("default");
    expect(keys.nameOf(alice)).toBe("alice");
    expect(keys.nameOf("sk-hum-nope")).toBeUndefined();
    expect(removeKey("alice")).toBe(true);
    expect(keys.nameOf(alice)).toBeUndefined();
  });

  test("with no key at all, anyone calls as anonymous", () => {
    expect(new Keys("").nameOf("")).toBe("anonymous");
  });
});

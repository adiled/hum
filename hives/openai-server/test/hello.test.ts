// What the hive's foragers and its scent attendant say of themselves in
// their hellos: a propensity in the documented vocabulary, and every chi
// each sounds and hears.

import { describe, expect, test } from "bun:test";
import { isValidChi } from "../../../thrum-clients/ts/index.ts";
import { HELLO as FORAGER } from "../src/requester.ts";
import { HELLO as SCENT } from "../src/scent.ts";

const STATEFULNESS = ["stateful", "convention-stateful", "stateless", "transport-only"];
const RICHNESS = ["rich", "medium", "lean", "opaque"];

function propensity(hello: { propensity?: unknown }): { statefulness: string; richness: string; wire: string } {
  const p = hello.propensity as { statefulness: string; richness: string; wire: string };
  expect(STATEFULNESS).toContain(p?.statefulness);
  expect(RICHNESS).toContain(p?.richness);
  expect(p?.wire).toMatch(/^openai\/[a-z-]+$/);
  return p;
}

describe("hellos", () => {
  test("a key's forager keeps the conversation and carries it whole", () => {
    expect(propensity(FORAGER)).toEqual({ statefulness: "stateful", richness: "rich", wire: "openai/api" });
  });

  test("a key's forager lists every chi it sounds and hears", () => {
    const chis = FORAGER.chis ?? [];
    for (const chi of ["hello", "prompt", "cancel", "cleanup", "unfurl", "wilt", "error", "tendril-reach", "tendril-info", "tendril-result"]) {
      expect(chis).toContain(chi);
    }
    expect(chis.every(isValidChi)).toBe(true);
  });

  test("scent distils each reach on its own, texts in and scents out", () => {
    expect(propensity(SCENT)).toEqual({ statefulness: "stateless", richness: "lean", wire: "openai/embeddings" });
    expect([...(SCENT.chis ?? [])].sort()).toEqual(["hello", "tendril-reach", "tendril-result"]);
  });
});

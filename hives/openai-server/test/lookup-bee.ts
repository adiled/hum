// An attendant for the end-to-end tests: one tool, `lookup`, that finds
// what it is asked for, and fails to find "nothing".

import { Bee, Chi, Role } from "../../../thrum-clients/ts/index.ts";

const bee = new Bee({
  hive: "openai-server",
  as: "lab-lookup",
  role: Role.attendant,
  hello: { tools: [{ name: "lookup", description: "Look a thing up.", inputSchema: { type: "object", properties: { q: { type: "string" } } } }] },
});
bee.on(Chi.tendrilReach, (t) => {
  const q = (t.args as { q?: string } | undefined)?.q ?? "";
  const found = q === "nothing" ? { output: "nothing found", isError: true } : { output: `found ${q}` };
  bee.send({ chi: "tendril-result", callId: t.callId, ...(t.sid ? { sid: t.sid } : {}), ...found });
});
await bee.start();
console.log("lookup nestled");
await bee.closed().catch(() => {});

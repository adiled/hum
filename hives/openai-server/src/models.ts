// GET /v1/models and GET /v1/models/{id}.
//
// The models a caller can use are what its breath says it can reach: the
// models nurses here and on trusted peers grow, and the scents attendants
// distil. Each is listed once, owned by the hive that serves it. A model
// that grows words also lists, as `tools`, the function tools the server
// runs for its blooms itself, so a client knows not to offer or run its
// own under those names.

import type { ServerResponse } from "node:http";
import type { Caller } from "./hive.ts";
import { HttpError, sendJson } from "./http.ts";

interface Listed {
  id: string;
  object: "model";
  created: number;
  owned_by: string;
  tools?: unknown[];
}

function listed(who: Caller): Listed[] {
  const served = (who.rq.reach.tools as Array<{ name?: string; description?: string; inputSchema?: unknown }>)
    .filter((t) => typeof t?.name === "string")
    .map((t) => ({ type: "function", function: { name: t.name, description: t.description ?? "", parameters: t.inputSchema ?? { type: "object", properties: {} } } }));
  const seen = new Map<string, Listed>();
  for (const m of who.rq.reach.models) {
    if (!seen.has(m.id)) seen.set(m.id, { id: m.id, object: "model", created: 0, owned_by: m.hive, ...(served.length > 0 ? { tools: served } : {}) });
  }
  for (const m of who.rq.reach.scents) {
    if (!seen.has(m.id)) seen.set(m.id, { id: m.id, object: "model", created: 0, owned_by: m.hive });
  }
  return [...seen.values()];
}

export function models(who: Caller, res: ServerResponse): void {
  sendJson(res, 200, { object: "list", data: listed(who) });
}

export function model(who: Caller, id: string, res: ServerResponse): void {
  const found = listed(who).find((m) => m.id === id);
  if (!found) throw new HttpError(404, `The model '${id}' does not exist, or nothing here serves it`, "model_not_found", "model");
  sendJson(res, 200, found);
}

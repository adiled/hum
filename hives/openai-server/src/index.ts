// openai-server: OpenAI's HTTP surface in front of hum, and the bees behind
// it that grow blooms and distil scents through an OpenAI-compatible API.
//
//   node dist/index.js                     serve
//   node dist/index.js key add <name>      make a key; its token is printed once
//   node dist/index.js key list
//   node dist/index.js key remove <name>
//
// The surface, each route also without its /v1:
//
//   GET    /v1/models, /v1/models/{id}
//   POST   /v1/chat/completions, /v1/responses, /v1/completions, /v1/embeddings
//   GET    /v1/responses/{id}       DELETE /v1/responses/{id}
//   GET    /v1/usage                what the calling key has taken
//
// A request bears a key once keys exist (keys.ts), and its key's forager
// bee asks humd for it, so any nurse humd can reach grows its blooms and
// any attendant distils its scents. A key may be held to a rate in the
// hive's config. Browsers may call from the origins the config allows.
//
// Behind the surface the hive's own bees serve what the API upstream
// offers: the nurse `main` grows its models' blooms and the attendant
// `scent` distils its embedding models' scents, both following the
// upstream's model list. If either is refused, the process ends, for its
// unit to restart it.

import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { chat } from "./chat.ts";
import { completions } from "./completions.ts";
import { loadConfig, type Rate } from "./config.ts";
import { Sigils } from "./conversation.ts";
import { embeddings } from "./embeddings.ts";
import type { Caller, Ctx } from "./hive.ts";
import { bearerOf, cors, HttpError, sendError, sendJson } from "./http.ts";
import { addKey, Keys, labelOf, listKeys, removeKey } from "./keys.ts";
import { model, models } from "./models.ts";
import { Nurse } from "./nurse.ts";
import { Requesters } from "./requester.ts";
import { dropResponse, readResponse, responses } from "./responses.ts";
import { Scent } from "./scent.ts";
import { watchModels } from "./upstream.ts";
import { Usage } from "./usage.ts";

function keyCommand(args: string[]): number {
  const [verb, name] = args;
  try {
    if (verb === "add" && name) {
      console.log(addKey(name));
      return 0;
    }
    if (verb === "list") {
      for (const k of listKeys()) console.log(`${k.name}\t${k.created}`);
      return 0;
    }
    if (verb === "remove" && name) {
      if (removeKey(name)) return 0;
      console.error(`no key named ${name}`);
      return 1;
    }
  } catch (e) {
    console.error((e as Error).message);
    return 1;
  }
  console.error("usage: key add <name> | key list | key remove <name>");
  return 2;
}

class Buckets {
  readonly #rate: Rate;
  readonly #held = new Map<string, { tokens: number; at: number }>();

  constructor(rate: Rate) {
    this.#rate = rate;
  }

  take(key: string): void {
    const now = Date.now();
    const b = this.#held.get(key) ?? { tokens: this.#rate.capacity, at: now };
    b.tokens = Math.min(this.#rate.capacity, b.tokens + ((now - b.at) / 1000) * this.#rate.refillPerSec);
    b.at = now;
    this.#held.set(key, b);
    if (b.tokens < 1) {
      throw new HttpError(429, `the key ${key} is past its rate; try again in ${Math.ceil((1 - b.tokens) / this.#rate.refillPerSec)} s`, "rate_limit_exceeded");
    }
    b.tokens -= 1;
  }
}

async function serve(): Promise<void> {
  const config = loadConfig();
  const keys = new Keys(config.apiKey);
  const requesters = new Requesters();
  const buckets = config.rate ? new Buckets(config.rate) : undefined;
  const ctx: Ctx = {
    config,
    requesters,
    usage: new Usage(),
    sigils: new Sigils((sid, label) => requesters.peek(label)?.send({ chi: "cleanup", sid })),
  };

  const route = async (req: IncomingMessage, res: ServerResponse): Promise<void> => {
    const url = new URL(req.url ?? "/", "http://hum");
    const path = url.pathname.replace(/\/+$/, "").replace(/^\/v1(?=\/|$)/, "") || "/";
    const allowed = cors(req, res, config.cors);
    if (req.method === "OPTIONS") {
      res.writeHead(allowed ? 204 : 403);
      res.end();
      return;
    }
    if (req.method === "GET" && path === "/") {
      res.writeHead(200, { "Content-Type": "text/plain" });
      res.end("hum-openai-server\n");
      return;
    }
    const key = keys.nameOf(bearerOf(req));
    if (!key) throw new HttpError(401, "Incorrect API key provided: bear one of this server's keys as `Authorization: Bearer <key>`", "invalid_api_key");
    buckets?.take(key);
    const who: Caller = { key, rq: await requesters.get(labelOf(key)) };
    const at = path.match(/^\/(models|responses)\/(.+)$/);
    switch (`${req.method} ${at ? `/${at[1]}/:id` : path}`) {
      case "GET /models":
        return models(who, res);
      case "GET /models/:id":
        return model(who, decodeURIComponent(at![2]), res);
      case "POST /chat/completions":
        return chat(ctx, who, req, res);
      case "POST /responses":
        return responses(ctx, who, req, res);
      case "GET /responses/:id":
        return readResponse(who, decodeURIComponent(at![2]), res);
      case "DELETE /responses/:id":
        return dropResponse(who, decodeURIComponent(at![2]), res);
      case "POST /completions":
        return completions(ctx, who, req, res);
      case "POST /embeddings":
        return embeddings(ctx, who, req, res);
      case "GET /usage":
        return sendJson(res, 200, { key, usage: ctx.usage.of(key) });
    }
    throw new HttpError(404, `${req.method} ${url.pathname} is not served here`, "unknown_url");
  };

  const server = createServer((req, res) => {
    route(req, res).catch((e) => {
      if (!res.headersSent) sendError(res, e);
      else if (!res.writableEnded) res.end();
    });
  });
  server.requestTimeout = 0;
  server.headersTimeout = 60_000;
  await new Promise<void>((resolve) => server.listen(config.port, config.host, resolve));
  const addr = server.address();
  const host = addr && typeof addr === "object" ? addr.address : config.host;
  const port = addr && typeof addr === "object" ? addr.port : config.port;
  console.log(`[hum-openai-server] listening on http://${host}:${port}${keys.open() ? " (no keys: anyone may call)" : ""}`);

  const nurse = new Nurse(undefined, { host, port, scheme: "http" });
  const scent = new Scent();
  for (const [bee, name] of [[nurse.bee, "nurse"], [scent.bee, "scent attendant"]] as const) {
    bee.start().then(() => console.log(`[hum-openai-server] the ${name} nestled as ${bee.id}`), () => {});
    bee.closed().catch((e) => {
      console.error(`[hum-openai-server] the ${name} ended: ${e}`);
      process.exit(1);
    });
  }
  watchModels(config.scents, (m) => {
    console.log(`[hum-openai-server] upstream grows ${m.grown.length} models and distils ${m.scents.length} scents`);
    nurse.serve(m.grown);
    scent.serve(m.scents);
  });
}

const [command, ...rest] = process.argv.slice(2);
if (command === "key") {
  process.exit(keyCommand(rest));
} else {
  serve().catch((e) => {
    console.error("[hum-openai-server] could not start:", e);
    process.exit(1);
  });
}

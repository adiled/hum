// The HTTP plumbing every route shares.
//
// Errors go out in Anthropic's envelope, `{ type: "error", error: { type,
// message } }`, its type the one Anthropic gives the status. A stream holds
// its headers and its `message_start` until it has something to say, so a
// request that fails before its first word still fails with its status,
// and a quiet stream is kept open with a `ping` every 15 seconds. Once a
// stream has begun, an error is its last event. A body past 64 MiB is
// refused.

import type { IncomingMessage, ServerResponse } from "node:http";

const BODY_MOST = 64 * 1024 * 1024;
const KEEPALIVE_MS = 15_000;

const TYPES: Record<number, string> = {
  400: "invalid_request_error",
  401: "authentication_error",
  403: "permission_error",
  404: "not_found_error",
  413: "request_too_large",
  429: "rate_limit_error",
  504: "timeout_error",
  529: "overloaded_error",
};

export class HttpError extends Error {
  readonly status: number;
  readonly type: string;

  constructor(status: number, message: string) {
    super(message);
    this.status = status;
    this.type = TYPES[status] ?? (status >= 500 ? "api_error" : "invalid_request_error");
  }

  envelope(): { type: "error"; error: { type: string; message: string } } {
    return { type: "error", error: { type: this.type, message: this.message } };
  }
}

export const bad = (message: string) => new HttpError(400, message);

export async function readJson<T>(req: IncomingMessage): Promise<T> {
  const body = await new Promise<string>((resolve, reject) => {
    const chunks: Buffer[] = [];
    let size = 0;
    req.on("data", (c: Buffer) => {
      size += c.length;
      if (size > BODY_MOST) {
        reject(new HttpError(413, `the body is past ${BODY_MOST} bytes`));
        req.destroy();
        return;
      }
      chunks.push(c);
    });
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    req.on("error", reject);
  });
  try {
    const parsed = JSON.parse(body);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) throw new Error("not an object");
    return parsed as T;
  } catch {
    throw bad("the body is not a JSON object");
  }
}

export function sendJson(res: ServerResponse, status: number, body: unknown, headers: Record<string, string> = {}): void {
  if (res.headersSent) return;
  res.writeHead(status, { "Content-Type": "application/json", ...headers });
  res.end(JSON.stringify(body));
}

export function sendError(res: ServerResponse, e: unknown, headers: Record<string, string> = {}): void {
  const err = e instanceof HttpError ? e : new HttpError(500, e instanceof Error ? e.message : String(e));
  sendJson(res, err.status, err.envelope(), headers);
}

export function gone(res: ServerResponse, fn: () => void): void {
  res.on("close", () => {
    if (!res.writableFinished) fn();
  });
}

export class Sse {
  readonly #res: ServerResponse;
  readonly #opening: () => [string, unknown];
  #open = false;
  #ended = false;
  #ticker: NodeJS.Timeout;

  constructor(res: ServerResponse, opening: () => [string, unknown]) {
    this.#res = res;
    this.#opening = opening;
    this.#ticker = setInterval(() => this.event("ping", { type: "ping" }), KEEPALIVE_MS);
    this.#ticker.unref();
  }

  #begin(): void {
    if (this.#open || this.#ended) return;
    this.#open = true;
    this.#res.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      "Connection": "keep-alive",
      "X-Accel-Buffering": "no",
    });
    const [event, data] = this.#opening();
    this.#write(event, data);
  }

  #write(event: string, data: unknown): void {
    this.#res.write(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`);
  }

  event(event: string, data: unknown): void {
    if (this.#ended) return;
    this.#begin();
    this.#write(event, data);
  }

  fail(e: HttpError): void {
    if (this.#ended) return;
    if (!this.#open) {
      this.#ended = true;
      clearInterval(this.#ticker);
      sendJson(this.#res, e.status, e.envelope());
      return;
    }
    this.#write("error", e.envelope());
    this.end();
  }

  end(): void {
    if (this.#ended) return;
    this.#begin();
    this.#ended = true;
    clearInterval(this.#ticker);
    this.#res.end();
  }
}

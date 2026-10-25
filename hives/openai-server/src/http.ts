// The HTTP plumbing every endpoint shares.
//
// Errors go out in OpenAI's envelope, `{ error: { message, type, code,
// param } }`, with the status that says what went wrong. A stream holds
// its headers until it has something to say, so a request that fails
// before its first word still fails with its status, and a quiet stream
// is kept open with an SSE comment every 15 seconds. Once a stream has
// begun, an error is its last event. A body past 64 MiB is refused.

import type { IncomingMessage, ServerResponse } from "node:http";

export const BODY_MOST = 64 * 1024 * 1024;
const KEEPALIVE_MS = 15_000;

export class HttpError extends Error {
  readonly status: number;
  readonly type: string;
  readonly code: string | null;
  readonly param: string | null;

  constructor(status: number, message: string, code: string | null = null, param: string | null = null) {
    super(message);
    this.status = status;
    this.code = code;
    this.param = param;
    this.type = status === 401
      ? "authentication_error"
      : status === 404
        ? "not_found_error"
        : status === 429
          ? "rate_limit_error"
          : status >= 500
            ? "server_error"
            : "invalid_request_error";
  }

  envelope(): { error: { message: string; type: string; code: string | null; param: string | null } } {
    return { error: { message: this.message, type: this.type, code: this.code, param: this.param } };
  }
}

export const bad = (message: string, param: string | null = null) => new HttpError(400, message, null, param);

export function readBody(req: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
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
}

export async function readJson<T>(req: IncomingMessage): Promise<T> {
  const body = await readBody(req);
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

export function sendError(res: ServerResponse, e: unknown): void {
  const err = e instanceof HttpError ? e : new HttpError(500, e instanceof Error ? e.message : String(e));
  sendJson(res, err.status, err.envelope(), err.status === 401 ? { "WWW-Authenticate": "Bearer" } : {});
}

export function gone(res: ServerResponse, fn: () => void): void {
  res.on("close", () => {
    if (!res.writableFinished) fn();
  });
}

export class Sse {
  readonly #res: ServerResponse;
  #open = false;
  #ended = false;
  #ticker: NodeJS.Timeout;

  constructor(res: ServerResponse) {
    this.#res = res;
    this.#ticker = setInterval(() => {
      this.#begin();
      this.#res.write(": keepalive\n\n");
    }, KEEPALIVE_MS);
    this.#ticker.unref();
  }

  get begun(): boolean {
    return this.#open;
  }

  get ended(): boolean {
    return this.#ended;
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
  }

  data(body: unknown, event?: string): void {
    if (this.#ended) return;
    this.#begin();
    this.#res.write(`${event ? `event: ${event}\n` : ""}data: ${typeof body === "string" ? body : JSON.stringify(body)}\n\n`);
  }

  fail(e: HttpError, event?: string): void {
    if (this.#ended) return;
    if (!this.#open) {
      this.#ended = true;
      clearInterval(this.#ticker);
      sendJson(this.#res, e.status, e.envelope());
      return;
    }
    const { code, message, param } = e.envelope().error;
    this.data(event ? { type: "error", code, message, param } : e.envelope(), event);
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

export function cors(req: IncomingMessage, res: ServerResponse, allowed: string[]): boolean {
  const origin = req.headers.origin;
  if (!origin || allowed.length === 0) return false;
  if (!allowed.includes("*") && !allowed.includes(origin)) return false;
  res.setHeader("Access-Control-Allow-Origin", allowed.includes("*") ? "*" : origin);
  res.setHeader("Vary", "Origin");
  res.setHeader("Access-Control-Allow-Headers", req.headers["access-control-request-headers"] ?? "authorization, content-type");
  res.setHeader("Access-Control-Allow-Methods", "GET, POST, DELETE, OPTIONS");
  res.setHeader("Access-Control-Max-Age", "600");
  return true;
}

export function bearerOf(req: IncomingMessage): string {
  const auth = req.headers.authorization;
  if (typeof auth === "string") {
    const m = auth.match(/^Bearer\s+(.+)$/i);
    if (m) return m[1].trim();
  }
  const key = req.headers["api-key"];
  return typeof key === "string" ? key.trim() : "";
}

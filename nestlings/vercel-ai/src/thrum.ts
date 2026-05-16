// Minimal thrum client for the Vercel AI nestling.
// Mirrors the openai-server client: connect to hum's NDJSON socket, send
// framed tones, dispatch incoming tones to per-sid subscribers.

import { createConnection, type Socket } from "node:net";

// Thrum protocol version this nestling targets. Bump when adopting a new
// chi or envelope field that the daemon must understand.
export const THRUM_VERSION = "0.1.0";
export const NESTLING_NAME = "vercel-ai";

export type Tone = Record<string, unknown>;
export type SidHandler = (msg: Tone) => void;

function defaultThrumPath(): string {
  const runtime = process.env.XDG_RUNTIME_DIR;
  const base = runtime ? `${runtime}/hum/hum.sock`
                       : `/tmp/hum-${process.getuid?.() ?? 0}/hum.sock`;
  return base + ".thrum";
}

export class ThrumClient {
  private sock: Socket | null = null;
  private buf = "";
  private byId = new Map<string, SidHandler>();
  private path: string;
  private connected = false;
  private pending: string[] = [];
  private connecting: Promise<void> | null = null;

  constructor(path?: string) {
    this.path = path ?? process.env.HUM_THRUM_PATH ?? defaultThrumPath();
  }

  async connect(): Promise<void> {
    if (this.connected) return;
    if (this.connecting) return this.connecting;
    this.connecting = new Promise<void>((resolve, reject) => {
      const s = createConnection(this.path);
      s.on("connect", () => {
        this.sock = s;
        this.connected = true;
        this.connecting = null;
        // Announce ourselves before flushing pending. Daemon traces
        // version mismatches but does not yet reject.
        s.write(JSON.stringify({
          chi: "hello",
          rid: `hello-${Date.now().toString(36)}`,
          from: NESTLING_NAME,
          nestling: NESTLING_NAME,
          protoVersion: THRUM_VERSION,
        }) + "\n");
        for (const line of this.pending) s.write(line);
        this.pending = [];
        resolve();
      });
      s.on("data", (chunk: Buffer) => {
        this.buf += chunk.toString();
        let nl: number;
        while ((nl = this.buf.indexOf("\n")) >= 0) {
          const line = this.buf.slice(0, nl);
          this.buf = this.buf.slice(nl + 1);
          if (!line) continue;
          try {
            const msg = JSON.parse(line) as Tone;
            const sid = (msg.sid as string) ?? "";
            const handler = this.byId.get(sid);
            if (handler) handler(msg);
          } catch { /* malformed — ignore */ }
        }
      });
      s.on("error", (err) => {
        this.connecting = null;
        if (!this.connected) reject(err);
      });
      s.on("close", () => {
        this.connected = false;
        this.sock = null;
      });
    });
    return this.connecting;
  }

  send(msg: Tone): void {
    const line = JSON.stringify(msg) + "\n";
    if (this.connected && this.sock) this.sock.write(line);
    else this.pending.push(line);
  }

  on(sid: string, handler: SidHandler): void { this.byId.set(sid, handler); }
  off(sid: string): void { this.byId.delete(sid); }
}

// Process-wide shared client. Vercel AI consumers instantiate many
// HumModels per session, but one daemon connection is plenty.
let shared: ThrumClient | null = null;
export function getThrum(): ThrumClient {
  if (!shared) shared = new ThrumClient();
  return shared;
}

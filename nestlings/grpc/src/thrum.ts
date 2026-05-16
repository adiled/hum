// Minimal thrum client.  Each gRPC bidi stream gets one of these — keeps
// sid handler maps isolated so concurrent gRPC clients can use overlapping
// sids without colliding.

import { createConnection, type Socket } from "node:net";

export const THRUM_VERSION = "0.1.0";
export const NESTLING_NAME = "grpc";

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
  private wildcard: SidHandler | null = null;
  private path: string;
  private connected = false;
  private pending: string[] = [];

  constructor(path?: string) {
    this.path = path ?? process.env.HUM_THRUM_PATH ?? defaultThrumPath();
  }

  async connect(): Promise<void> {
    if (this.connected) return;
    return new Promise<void>((resolve, reject) => {
      const s = createConnection(this.path);
      s.on("connect", () => {
        this.sock = s;
        this.connected = true;
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
            const handler = (sid && this.byId.get(sid)) || this.wildcard;
            if (handler) handler(msg);
          } catch { /* malformed — ignore */ }
        }
      });
      s.on("error", (err) => {
        if (!this.connected) reject(err);
      });
      s.on("close", () => {
        this.connected = false;
        this.sock = null;
      });
    });
  }

  send(msg: Tone): void {
    const line = JSON.stringify(msg) + "\n";
    if (this.connected && this.sock) this.sock.write(line);
    else this.pending.push(line);
  }

  on(sid: string, handler: SidHandler): void { this.byId.set(sid, handler); }
  off(sid: string): void { this.byId.delete(sid); }
  // Single "catch everything not routed by sid" handler — used for breath,
  // echo, pulse and other tones that may arrive without a sid.
  onAny(handler: SidHandler): void { this.wildcard = handler; }
  close(): void { try { this.sock?.end(); } catch { /* ignore */ } this.sock = null; this.connected = false; }
}

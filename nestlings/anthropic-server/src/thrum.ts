// Minimal thrum client. Connects to hum's NDJSON socket, sends framed
// tones, dispatches incoming tones to subscribers by `sid`.
//
// Shape mirrors nestlings/openai-server/src/thrum.ts — same wire, just
// a different nestling name.

import { createConnection, type Socket } from "node:net";

export const THRUM_VERSION = "0.7.0";
export const NESTLING_NAME = "anthropic-server";

export type Tone = Record<string, unknown>;
export type SidHandler = (msg: Tone) => void;

export interface BindInfo {
  host: string;
  port: number;
  scheme: string;
}

function defaultThrumPath(): string {
  const sock = process.env.HUM_THRUM_SOCK ?? process.env.HUM_THRUM_PATH;
  if (sock) return sock;
  const runtime = process.env.XDG_RUNTIME_DIR ?? `/run/user/${process.getuid?.() ?? 1000}`;
  return `${runtime}/hum/thrum.sock`;
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
    this.path = path ?? defaultThrumPath();
  }

  async connect(bind?: BindInfo): Promise<void> {
    return new Promise((resolve, reject) => {
      const s = createConnection(this.path);
      s.on("connect", () => {
        this.sock = s;
        this.connected = true;
        // hello — humd reads `nestling`, `version`, `protoVersion`,
        // `propensity`, `chi`, `source` to build a NestlingManifest
        // and gossip it on hum/nestlings/announce. See WIRE.md §Handshake.
        const hello: Tone = {
          chi: "hello",
          rid: `hello-${Date.now().toString(36)}`,
          from: NESTLING_NAME,
          nestling: NESTLING_NAME,
          version: "0.0.0",
          protoVersion: THRUM_VERSION,
          propensity: {
            statefulness: "convention-stateful",
            richness:     "medium",
            wire:         "anthropic/messages",
          },
          chis: ["hello", "prompt", "cancel", "chunk", "finish", "error", "tool-call", "tool-result"],
          source: "https://github.com/adiled/hum/tree/main/nestlings/anthropic-server",
        };
        if (bind) hello.bind = bind;
        s.write(JSON.stringify(hello) + "\n");
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
  onAny(handler: SidHandler): void { this.wildcard = handler; }
}

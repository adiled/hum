// Minimal thrum client. Connects to hum's NDJSON socket, sends framed
// tones, dispatches incoming tones to subscribers by `sid`.
//
// Shape mirrors hives/openai-server/src/thrum.ts — same wire, just
// a different nestling name.

import { createConnection, type Socket } from "node:net";
import { beeHid } from "./identity";

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
  private bind?: BindInfo;
  private shuttingDown = false;
  private reconnectAttempt = 0;
  private reconnectTimer: NodeJS.Timeout | null = null;

  constructor(path?: string) {
    this.path = path ?? defaultThrumPath();
  }

  async connect(bind?: BindInfo): Promise<void> {
    this.bind = bind;
    return new Promise((resolve, reject) => {
      this.attempt(resolve, reject);
    });
  }

  private attempt(
    resolve?: () => void,
    reject?: (e: unknown) => void,
  ): void {
    const s = createConnection(this.path);
    let settled = false;
    s.on("connect", () => {
      this.sock = s;
      this.connected = true;
      this.reconnectAttempt = 0;
      // hello — humd reads `nestling`, `version`, `protoVersion`,
      // `propensity`, `chi`, `source` to build a NestlingManifest
      // and gossip it on hum/hives/announce. See WIRE.md §Handshake.
      const hello: Tone = {
        chi: "hello",
        rid: `hello-${Date.now().toString(36)}`,
        from: NESTLING_NAME,
        hid: beeHid(NESTLING_NAME, "fbee"),
        bee: ["forager"],
        hive: NESTLING_NAME,
        provides: ["session"],
        nestling: NESTLING_NAME,
        version: "0.0.0",
        protoVersion: THRUM_VERSION,
        propensity: {
          statefulness: "convention-stateful",
          richness:     "medium",
          wire:         "anthropic/messages",
        },
        chis: ["hello", "prompt", "cancel", "chunk", "finish", "error", "tool-call", "tool-result"],
        source: "https://github.com/adiled/hum/tree/main/hives/anthropic-server",
      };
      if (this.bind) hello.bind = this.bind;
      s.write(JSON.stringify(hello) + "\n");
      for (const line of this.pending) s.write(line);
      this.pending = [];
      if (!settled && resolve) { settled = true; resolve(); }
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
      if (!settled && !this.connected && reject) {
        settled = true;
        reject(err);
      }
    });
    s.on("close", () => {
      const wasConnected = this.connected;
      this.connected = false;
      this.sock = null;
      if (this.shuttingDown) return;
      if (wasConnected) console.error("[thrum] socket closed; reconnecting…");
      this.scheduleReconnect();
    });
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer) return;
    const delay = Math.min(30_000, 250 * Math.pow(2, this.reconnectAttempt));
    this.reconnectAttempt++;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.attempt();
    }, delay);
  }

  send(msg: Tone): void {
    const line = JSON.stringify(msg) + "\n";
    if (this.connected && this.sock) this.sock.write(line);
    else this.pending.push(line);
  }

  on(sid: string, handler: SidHandler): void { this.byId.set(sid, handler); }
  off(sid: string): void { this.byId.delete(sid); }
  onAny(handler: SidHandler): void { this.wildcard = handler; }

  close(): void {
    this.shuttingDown = true;
    if (this.reconnectTimer) { clearTimeout(this.reconnectTimer); this.reconnectTimer = null; }
    if (this.sock) { this.sock.end(); this.sock = null; }
    this.connected = false;
  }
}

import { createConnection, type Socket } from "node:net";
import { beeHid } from "./identity";

export const THRUM_VERSION = "0.7.0";
export const HIVE_NAME = "openai-server";
export const BEE_VERSION = "0.31.3";
export const BEE_ROLE = "forager";
export const BEE_PROVIDES = ["session"];

// Minimal thrum client. Connects to hum's NDJSON socket, sends framed
// tones, dispatches incoming tones to subscribers by `sid`.

export type Tone = Record<string, unknown>;
export type SidHandler = (msg: Tone) => void;

export interface BindInfo {
  host: string;
  port: number;
  scheme: string;
}

function defaultThrumPath(): string {
  // Canonical resolution mirrors thrumd::default_socket_path() in Rust.
  // HUM_THRUM_SOCK wins; HUM_SOCKET is the legacy fallback so an
  // in-flight upgrade doesn't strand nestlings.
  const explicit = process.env.HUM_THRUM_SOCK ?? process.env.HUM_SOCKET;
  if (explicit) return explicit;
  const runtime = process.env.XDG_RUNTIME_DIR
    ?? `/tmp/hum-${process.getuid?.() ?? 0}`;
  return `${runtime}/hum/thrum.sock`;
}

export class ThrumClient {
  private sock: Socket | null = null;
  private buf = "";
  private byId = new Map<string, SidHandler>();
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
    // First-connect resolves on hello write; subsequent reconnects
    // are silent (driven by the close handler's backoff loop).
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
      const hello: Tone = {
        chi: "hello",
        rid: `hello-${Date.now().toString(36)}`,
        from: HIVE_NAME,
        hid: beeHid(HIVE_NAME, "fbee"),
        bee: [BEE_ROLE],
        hive: HIVE_NAME,
        version: BEE_VERSION,
        provides: BEE_PROVIDES,
        protoVersion: THRUM_VERSION,
        chis: ["hello", "prompt", "cancel", "tool-result", "chunk", "finish", "session-ready", "tool-call", "error"],
        source: "https://github.com/adiled/hum/tree/main/hives/openai-server",
      };
      if (this.bind) hello.bind = this.bind;
      s.write(JSON.stringify(hello) + "\n");
      // Flush anything queued during the disconnect window.
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
          const handler = this.byId.get(sid);
          if (handler) handler(msg);
        } catch {}
      }
    });
    s.on("error", (err) => {
      // Only reject if we never connected on this attempt; otherwise
      // let the `close` handler schedule a reconnect.
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
      if (wasConnected) {
        console.error("[thrum] socket closed; reconnecting…");
      }
      this.scheduleReconnect();
    });
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer) return;
    // Exponential backoff capped at 30s. Pending writes queue
    // forward — they ship on the next successful connect.
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

  close(): void {
    this.shuttingDown = true;
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.sock) {
      this.sock.end();
      this.sock = null;
    }
    this.connected = false;
  }
}

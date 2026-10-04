import { createConnection, type Socket } from "node:net";
import { beeHid } from "./identity";
import pkg from "../package.json";

export const THRUM_VERSION = "0.7.0";
export const HIVE_NAME = "openai-server";
export const BEE_VERSION = pkg.version;
export const BEE_ROLE = "forager";
export const BEE_ROLES = ["forager", "worker"];
export const BEE_PROVIDES = ["session"];
export const BEE_MODELS: string[] = (process.env.OPENAI_WORKER_MODELS ?? "")
  .split(",").map(s => s.trim()).filter(s => s.length > 0);

const RECONNECT_BASE_MS = 250;
const RECONNECT_CEILING_MS = 30_000;

function reconnectDelayMs(attempt: number): number {
  return Math.min(RECONNECT_CEILING_MS, RECONNECT_BASE_MS * Math.pow(2, attempt));
}

export type Tone = Record<string, unknown>;
export type SidHandler = (msg: Tone) => void;

export interface BindInfo {
  host: string;
  port: number;
  scheme: string;
}

function defaultThrumPath(): string {
  const explicit = process.env.HUM_THRUM_SOCK ?? process.env.HUM_SOCKET;
  if (explicit) return explicit;
  const stateHome = process.env.XDG_STATE_HOME
    ?? `${process.env.HOME ?? "/tmp"}/.local/state`;
  return `${stateHome}/hum/thrum.sock`;
}

export class ThrumClient {
  private sock: Socket | null = null;
  private buf = "";
  private byId = new Map<string, SidHandler>();
  private byChi = new Map<string, SidHandler>();
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
      const hello: Tone = {
        chi: "hello",
        rid: `hello-${Date.now().toString(36)}`,
        from: HIVE_NAME,
        hid: beeHid(HIVE_NAME, "fbee"),
        bee: BEE_ROLES,
        hive: HIVE_NAME,
        version: BEE_VERSION,
        provides: BEE_PROVIDES,
        models: BEE_MODELS,
        propensity: { statefulness: "stateless_per_call", wire: HIVE_NAME },
        protoVersion: THRUM_VERSION,
        chis: ["hello", "prompt", "cancel", "tool-result", "chunk", "finish", "session-ready", "tool-call", "error"],
        source: "https://github.com/adiled/hum/tree/main/hives/openai-server",
      };
      if (this.bind) hello.bind = this.bind;
      s.write(JSON.stringify(hello) + "\n");
      this.flushPending(s);
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
          if (handler) { handler(msg); continue; }
          const chi = (msg.chi as string) ?? "";
          const chiHandler = this.byChi.get(chi);
          if (chiHandler) chiHandler(msg);
        } catch {}
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
      if (wasConnected) {
        console.error("[thrum] socket closed; reconnecting…");
      }
      this.scheduleReconnect();
    });
  }

  private flushPending(s: Socket): void {
    for (const line of this.pending) s.write(line);
    this.pending = [];
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer) return;
    const delay = reconnectDelayMs(this.reconnectAttempt);
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

  onChi(chi: string, handler: SidHandler): void { this.byChi.set(chi, handler); }
  offChi(chi: string): void { this.byChi.delete(chi); }

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

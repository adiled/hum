import { createConnection, type Socket } from "node:net";

export const THRUM_VERSION = "0.7.0";
export const NESTLING_NAME = "openai-server";

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

  constructor(path?: string) {
    this.path = path ?? defaultThrumPath();
  }

  async connect(bind?: BindInfo): Promise<void> {
    return new Promise((resolve, reject) => {
      const s = createConnection(this.path);
      s.on("connect", () => {
        this.sock = s;
        this.connected = true;
        const hello: Tone = {
          chi: "hello",
          rid: `hello-${Date.now().toString(36)}`,
          from: NESTLING_NAME,
          nestling: NESTLING_NAME,
          protoVersion: THRUM_VERSION,
          source: "https://github.com/adiled/hum/tree/main/nestlings/openai-server",
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
            const handler = this.byId.get(sid);
            if (handler) handler(msg);
          } catch {}
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
}

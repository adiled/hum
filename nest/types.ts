// ─── Roost shapes ───────────────────────────────────────────────────────
//
// The daemon-side process abstraction: stdin/stdout/stderr + lifecycle.
// PipeRoost backs it with node:child_process; PtyRoost backs it with
// the PTY harness composing pty/* + hooks/* + transcript/*.

import type { Readable } from "node:stream";

export interface RoostProc {
  pid: number | undefined;
  stdin: { write(data: string): boolean };
  stdout: Readable;
  stderr: Readable;
  kill(signal?: number | string): void;
  exited: Promise<number>;
}

export interface BloomListener {
  sessionId: string;
  onRoost(claudeId: string, model: string, tools: string[]): void;
  onPetal(type: string, payload: Record<string, unknown>): void;
  onWilt(harvest: { finishReason: string; usage: Record<string, number> | undefined; providerMetadata: Record<string, unknown> }): void;
  onThorn(wound: string): void;
}

export interface Roost {
  proc: RoostProc;
  listeners: Map<string, BloomListener>;
  activeSid: string | null;
  ephemeral?: boolean;
  poolKey?: string;
}

export interface NestSession {
  needsRespawn?: boolean;
  claudeSessionId?: string | null;
}

export interface PermitHoldEntry {
  resolve: (decision: "allow" | "deny") => void;
  tool: string;
  path?: string;
  sessionId: string;
  createdAt: number;
}

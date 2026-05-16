import { existsSync } from "node:fs";
import { randomUUID } from "node:crypto";

import { trace, info } from "../log.ts";
import { sigil } from "../thrum/index.ts";
import { sessionPath as getSessionPath, sanitizeJsonl } from "../fs/session.ts";
import { setPermissions as mcpSetPerms, setAllowedTools as mcpSetAllowed, setCwd as mcpSetCwd } from "../mcp/tools.ts";
import type { HumConfig } from "../fs/config.ts";

import { pickPerch } from "./index.ts";
import { encodePrompt, encodeToolResult, parseLine } from "./protocol.ts";
import type { Roost, BloomListener, Hum, PermitHoldEntry, RoostProc } from "./types.ts";

export interface NestDeps {
  cfg: HumConfig;
  cliPath: string;
  mcpUrl: string;
  hums: Map<string, Hum>;
  saveHums: (sid?: string) => void;
  drift: {
    mark: (sigil: string, event: string) => void;
    span: (sigil: string, name: string, ms: number) => void;
  };
  drone: {
    observed: (sigil: string, event: Record<string, unknown>) => void;
  };
  thrum: (sessionId: string, msg: Record<string, unknown>) => void;
  thrumPulse: (kind: string, poolKey: string, payload?: Record<string, unknown>) => void;
  getPermissionAction: (tool: string, path?: string) => "allow" | "deny" | "ask";
  permitHold: Map<string, PermitHoldEntry>;
  recordPermitHoldSpan: (askId: string) => void;
}

export class Nest {
  private roosts = new Map<string, Roost>();
  private fadingRoosts = new Map<string, Promise<number>>();
  private idleTimers = new Map<string, ReturnType<typeof setTimeout>>();
  private streamedTurn = false;
  private reasoningBlockIdx: number | null = null;

  constructor(private readonly d: NestDeps) {}

  async awaken(
    poolKey: string,
    modelId: string,
    listener: BloomListener,
    resumeId?: string,
    permissions?: unknown[],
    systemPrompt?: string,
    allowedTools?: string[],
    sessionCwd?: string,
    planMode?: boolean,
  ): Promise<void> {
    const fading = this.fadingRoosts.get(poolKey);
    if (fading) {
      trace("nest.awaken.wait.fading", { poolKey });
      await fading.catch(() => {});
      await new Promise<void>(r => setTimeout(r, 100));
    }
    let roost = this.roosts.get(poolKey);

    const session = this.d.hums.get(poolKey);
    if (session?.needsRespawn) {
      if (roost) {
        trace("nest.respawn", { poolKey, reason: "seed" });
        this.d.thrumPulse("roost-died", poolKey, { pid: roost.proc.pid, reason: "respawn" });
        try { roost.proc.kill(); } catch {}
        this.roosts.delete(poolKey);
        roost = undefined;
      }
      session.needsRespawn = false;
      this.d.saveHums(poolKey);
    }

    const idleTimer = this.idleTimers.get(poolKey);
    if (idleTimer) {
      clearTimeout(idleTimer);
      this.idleTimers.delete(poolKey);
    }

    if (!roost) {
      if (this.roosts.size >= this.d.cfg.maxProcs) {
        let evictKey: string | null = null;
        for (const [key, r] of this.roosts) {
          if (r.listeners.size === 0 && r.activeSid === null) { evictKey = key; break; }
        }
        if (evictKey) {
          trace("nest.evicted", { poolKey: evictKey, reason: "maxProcs" });
          this.d.thrumPulse("roost-evicted", evictKey, { reason: "maxProcs" });
          try { this.roosts.get(evictKey)!.proc.kill(); } catch {}
          this.roosts.delete(evictKey);
          this.idleTimers.delete(evictKey);
        } else {
          trace("nest.rejected", { poolKey, reason: "maxProcs", active: this.roosts.size });
        }
      }
      const explicitNest = session?.nest?.[0]?.nest as "claude-repl" | "claude-cli" | undefined;
      roost = this.spawnProc(poolKey, modelId, resumeId || session?.nest?.[0]?.id || undefined, permissions, systemPrompt, allowedTools, sessionCwd, planMode, explicitNest);
    } else {
      mcpSetPerms((permissions ?? []) as any);
      mcpSetAllowed(allowedTools);
    }
    listener.onPetal("stream_start", {});
    roost.listeners.set(listener.sessionId, listener);
  }

  interrupt(poolKey: string): void {
    const roost = this.roosts.get(poolKey);
    if (!roost) return;
    if (roost.ephemeral) {
      trace("pty.stdin.ignored", { poolKey, type: "control_cancel_request" });
      return;
    }
    const requestId = randomUUID();
    roost.proc.stdin.write(JSON.stringify({
      type: "control_cancel_request",
      request_id: requestId,
    }) + "\n");
    trace("nest.interrupted", { poolKey, requestId });
  }

  murmur(sessionId: string, poolKey: string, content: Array<Record<string, unknown>> | string): void {
    const roost = this.roosts.get(poolKey);
    if (!roost?.proc.stdin) return;
    roost.activeSid = sessionId;
    const len = typeof content === "string" ? content.length : content.reduce((s, p) => s + ((p.text as string)?.length ?? 0), 0);
    trace("nest.murmured", { sid: sessionId, poolKey, len, parts: typeof content === "string" ? 1 : content.length });
    roost.proc.stdin.write(encodePrompt(content) + "\n");
  }

  reply(sessionId: string, poolKey: string, toolUseId: string, result: string): void {
    const roost = this.roosts.get(poolKey);
    if (!roost?.proc.stdin) return;
    roost.activeSid = sessionId;
    trace("nest.replied", { sid: sessionId, toolUseId, len: result.length });
    roost.proc.stdin.write(encodeToolResult(toolUseId, result) + "\n");
  }

  hush(sessionId: string, poolKey: string): void {
    const roost = this.roosts.get(poolKey);
    if (roost) {
      roost.listeners.delete(sessionId);
      if (roost.activeSid === sessionId) roost.activeSid = null;
      trace("nest.hushed", { sid: sessionId, poolKey });

      if (this.d.cfg.idleTimeout > 0 && roost.listeners.size === 0) {
        this.idleTimers.set(poolKey, setTimeout(() => {
          const r = this.roosts.get(poolKey);
          if (r && r.listeners.size === 0) {
            trace("nest.idle", { poolKey, pid: r.proc.pid, timeout: this.d.cfg.idleTimeout });
            this.d.thrumPulse("roost-idle", poolKey, { pid: r.proc.pid });
            try { r.proc.kill(); } catch {}
            this.roosts.delete(poolKey);
          }
          this.idleTimers.delete(poolKey);
        }, this.d.cfg.idleTimeout));
      }
    }
  }

  fell(sessionId: string, poolKey: string): void {
    const roost = this.roosts.get(poolKey);
    if (roost) {
      roost.listeners.delete(sessionId);
      if (roost.activeSid === sessionId) roost.activeSid = null;
      if (roost.listeners.size === 0) {
        trace("nest.felled", { poolKey, pid: roost.proc.pid });
        this.d.thrumPulse("roost-died", poolKey, { pid: roost.proc.pid, reason: "felled" });
        try { roost.proc.kill(); } catch {}
        this.roosts.delete(poolKey);
        const timer = this.idleTimers.get(poolKey);
        if (timer) { clearTimeout(timer); this.idleTimers.delete(poolKey); }
      }
    }
  }

  roost(poolKey: string): Roost | undefined {
    return this.roosts.get(poolKey);
  }

  survey(): Array<{ model: string; pid?: number; sessions: string[] }> {
    const out: Array<{ model: string; pid?: number; sessions: string[] }> = [];
    for (const [id, roost] of this.roosts) {
      out.push({ model: id, pid: roost.proc.pid, sessions: Array.from(roost.listeners.keys()) });
    }
    return out;
  }

  silence(): void {
    for (const [, roost] of this.roosts) { try { roost.proc.kill(); } catch {} }
    this.roosts.clear();
    for (const timer of this.idleTimers.values()) clearTimeout(timer);
    this.idleTimers.clear();
  }

  private spawnProc(
    poolKey: string,
    modelId: string,
    resumeId?: string,
    permissions?: unknown[],
    systemPrompt?: string,
    allowedTools?: string[],
    sessionCwd?: string,
    planMode?: boolean,
    explicitNest?: "claude-repl" | "claude-cli",
  ): Roost {
    mcpSetPerms((permissions ?? []) as any);
    mcpSetAllowed(allowedTools);
    if (sessionCwd) mcpSetCwd(sessionCwd);

    // No cwd → pure inference mode. Skip hum fs MCP entirely.
    const mcpServers: Record<string, { type: string; url: string }> = {};
    if (sessionCwd) {
      mcpServers.hum = { type: "http", url: `${this.d.mcpUrl}/s/${poolKey}` };
    }
    const mcpConfig = JSON.stringify({ mcpServers });

    const perch = pickPerch(this.d.cfg, sessionCwd, explicitNest);
    const usePty = perch.ephemeral;

    const sharedArgs = [
      "--verbose",
      "--model", modelId,
      "--dangerously-skip-permissions",
      "--disallowedTools", [
        "Read", "Edit", "Write", "MultiEdit", "ApplyPatch", "Bash", "Glob", "Grep",
        "ToolSearch", "NotebookEdit", "CodeSearch",
        "Agent", "Explore", "SendMessage",
        "EnterPlanMode", "ExitPlanMode", "EnterWorktree", "ExitWorktree",
        "AskUserQuestion",
        "TaskCreate", "TaskGet", "TaskList", "TaskUpdate", "TaskOutput", "TaskStop",
        "CronCreate", "CronDelete", "CronList", "Monitor", "RemoteTrigger", "ScheduleWakeup",
      ].join(","),
      "--mcp-config", mcpConfig,
      "--strict-mcp-config",
      "--disable-slash-commands",
    ];

    const cmd = usePty
      ? [this.d.cliPath, ...sharedArgs]
      : [this.d.cliPath, "-p", ...sharedArgs,
         "--input-format", "stream-json",
         "--output-format", "stream-json",
         "--include-partial-messages",
        ];
    if (systemPrompt) {
      cmd.push("--system-prompt", systemPrompt);
    }
    const spawnCwd = sessionCwd ?? process.env.HUM_CWD ?? process.env.HOME ?? "/";

    let effectiveResumeId: string | undefined = resumeId;
    if (resumeId) {
      const jsonlPath = getSessionPath(spawnCwd, resumeId);
      if (!existsSync(jsonlPath)) {
        trace("nest.resume.stale", { poolKey, resumeId, jsonlPath });
        effectiveResumeId = undefined;
      } else {
        try {
          const result = sanitizeJsonl(jsonlPath);
          if (result.fixed > 0) {
            trace("sanitize.applied", { poolKey, removed: result.removed, fixed: result.fixed, rules: result.rules });
          }
        } catch (e) {
          trace("sanitize.error", { poolKey, err: String(e) });
        }
      }
    }

    const harnessSessionId = usePty ? (effectiveResumeId || randomUUID()) : effectiveResumeId;
    if (effectiveResumeId) {
      cmd.push("--resume", effectiveResumeId);
    } else if (usePty && harnessSessionId) {
      cmd.push("--session-id", harnessSessionId);
    }

    const spawnEnv = {
      ...process.env,
      TERM: "xterm-256color",
      DIRENV_DISABLE: "1",
      ENABLE_TOOL_SEARCH: "false",
      CLAUDE_CODE_DISABLE_FAST_MODE: "1",
      DISABLE_INTERLEAVED_THINKING: "1",
      CLAUDE_CODE_DISABLE_CLAUDE_MDS: "1",
      CLAUDE_CODE_DISABLE_AUTO_MEMORY: "1",
      CLAUDE_CODE_DISABLE_BACKGROUND_TASKS: "1",
      ...(planMode ? {} : { CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING: "1" }),
      ...this.d.cfg.ccFlags,
    };

    const transcriptPath = harnessSessionId ? getSessionPath(spawnCwd, harnessSessionId) : undefined;
    const sigilStr = sigil(poolKey);
    const roostProc: RoostProc = perch.spawn({
      command: cmd[0],
      args: cmd.slice(1),
      cwd: spawnCwd,
      env: spawnEnv as Record<string, string>,
      harnessSessionId,
      transcriptPath,
      onPerfMark: (event, span) => {
        this.d.drift.mark(sigilStr, event);
        if (span) this.d.drift.span(sigilStr, span.name, span.ms);
      },
    });

    const roost: Roost = { proc: roostProc, listeners: new Map(), activeSid: null, ephemeral: perch.ephemeral, poolKey };
    this.roosts.set(poolKey, roost);
    info("nest.awakened", { poolKey, model: modelId, pid: roostProc.pid, usePty, resume: resumeId ?? "none" });
    this.d.thrumPulse("roost-spawned", poolKey, { pid: roostProc.pid });

    if (!perch.ephemeral) {
      this.readStderr(roostProc, poolKey);
    }

    roostProc.exited.then(code => {
      trace("nest.exited", { poolKey, code, pid: roostProc.pid });
      const current = this.roosts.get(poolKey);
      if (current === roost) {
        this.d.thrumPulse("roost-died", poolKey, { pid: roostProc.pid, reason: `exit:${code}` });
        for (const listener of roost.listeners.values()) {
          try { listener.onThorn(`subprocess exited: code=${code}`); } catch {}
        }
        roost.listeners.clear();
        roost.activeSid = null;
        this.roosts.delete(poolKey);
      } else {
        trace("nest.exited.stale", { poolKey, pid: roostProc.pid, reason: "replaced by newer roost" });
      }
    });

    this.readLoop(roostProc, poolKey, roost);
    return roost;
  }

  private readStderr(proc: RoostProc, modelId: string): void {
    proc.stderr.on("data", (chunk: Buffer) => {
      const text = chunk.toString().trim();
      if (text) trace("nest.stderr", { poolKey: modelId, text });
    });
  }

  private readLoop(proc: RoostProc, poolKey: string, roost: Roost): void {
    let nectar = "";
    proc.stdout.on("data", (chunk: Buffer) => {
      nectar += chunk.toString();
      const lines = nectar.split("\n");
      nectar = lines.pop() ?? "";
      for (const line of lines) {
        if (line.trim()) this.dispatchLine(poolKey, roost, parseLine(line));
      }
    });
    proc.stdout.on("error", (err) => {
      trace("nest.readloop.failed", { err: String(err) });
      for (const listener of roost.listeners.values()) {
        try { listener.onThorn(`readLoop error: ${err}`); } catch {}
      }
      roost.listeners.clear();
    });
  }

  private dispatchLine(poolKey: string, roost: Roost, raw: unknown): void {
    if (!raw || typeof raw !== "object") return;
    let msg = raw as Record<string, unknown>;

    if (msg.type === "stream_event" && msg.event && typeof msg.event === "object") {
      msg = msg.event as Record<string, unknown>;
    }

    if (msg.type === "system" && msg.subtype === "init") {
      const sid = (msg.session_id as string) ?? "";
      const model = (msg.model as string) ?? poolKey;
      const tools = ((msg.tools as unknown[]) ?? []).map(String);
      for (const listener of roost.listeners.values()) listener.onRoost(sid, model, tools);
      return;
    }

    let listener: BloomListener | undefined;
    if (roost.activeSid) {
      listener = roost.listeners.get(roost.activeSid);
    }
    if (!listener) {
      listener = roost.listeners.values().next().value;
    }
    if (!listener) return;

    const petal = (type: string, payload: Record<string, unknown>) => listener!.onPetal(type, payload);

    trace("stream.msg.received", { type: msg.type as string, subtype: (msg.subtype as string) ?? "" });

    if (msg.type === "permission_request") {
      const requestId = msg.request_id as string;
      const toolName = ((msg.tool_name ?? "") as string).replace("mcp__hum__", "");
      const path = ((msg.input as Record<string, unknown>)?.file_path ?? (msg.input as Record<string, unknown>)?.path) as string | undefined;
      if (roost.ephemeral) {
        trace("pty.stdin.ignored", { poolKey, type: "permission_response", requestId, tool: toolName });
        return;
      }
      const action = this.d.getPermissionAction(toolName, path);
      trace("permission.request.received", { requestId, tool: toolName, path, action });

      if (action === "deny") {
        trace("permission.denied", { requestId, tool: toolName, path });
        roost.proc.stdin?.write(JSON.stringify({
          type: "permission_response",
          request_id: requestId,
          subtype: "error",
          error: "Denied by session permission rules",
        }) + "\n");
      } else if (action === "ask") {
        const askId = requestId;
        trace("permission.hold.created", { id: askId, tool: toolName, path });
        this.d.drone.observed(sigil(poolKey), { type: "permission_ask" });

        this.d.thrum(roost.activeSid ?? "", {
          chi: "permission-ask",
          askId,
          tool: toolName,
          path,
          input: msg.input ?? {},
          dusk: Date.now() + this.d.cfg.permissionDusk,
        });

        this.d.permitHold.set(askId, {
          resolve: (decision) => {
            if (decision === "allow") {
              roost.proc.stdin?.write(JSON.stringify({
                type: "permission_response",
                request_id: requestId,
                subtype: "success",
                response: { updated_input: {}, permission_updates: [] },
              }) + "\n");
            } else {
              roost.proc.stdin?.write(JSON.stringify({
                type: "permission_response",
                request_id: requestId,
                subtype: "error",
                error: "Denied by user",
              }) + "\n");
            }
          },
          tool: toolName,
          path,
          sessionId: roost.activeSid ?? "",
          createdAt: Date.now(),
        });

        setTimeout(() => {
          if (this.d.permitHold.has(askId)) {
            this.d.recordPermitHoldSpan(askId);
            const hold = this.d.permitHold.get(askId)!;
            this.d.permitHold.delete(askId);
            hold.resolve("deny");
            trace("permission.hold.timeout", { id: askId });
          }
        }, this.d.cfg.permissionDusk);
      } else {
        trace("permission.allowed", { requestId, tool: toolName, path });
        roost.proc.stdin?.write(JSON.stringify({
          type: "permission_response",
          request_id: requestId,
          subtype: "success",
          response: { updated_input: {}, permission_updates: [] },
        }) + "\n");
      }
      return;
    }

    if (msg.type === "content_block_start") {
      const block = (msg.content_block ?? {}) as Record<string, unknown>;
      if (block.type === "thinking") {
        this.reasoningBlockIdx = msg.index as number;
        petal("reasoning_start", { id: msg.index });
      }
      if (block.type === "text") petal("text_start", { id: msg.index });
      if (block.type === "tool_use") {
        petal("tool_input_start", { toolCallId: block.id as string, toolName: block.name as string });
        this.d.drone.observed(sigil(poolKey), { type: "tool_start", toolName: block.name as string });
      }
      return;
    }

    if (msg.type === "content_block_delta") {
      this.streamedTurn = true;
      const delta = (msg.delta ?? {}) as Record<string, unknown>;
      if (delta.type === "thinking_delta") petal("reasoning_delta", { delta: delta.thinking as string });
      if (delta.type === "text_delta") {
        petal("text_delta", { delta: delta.text as string });
        this.d.drone.observed(sigil(poolKey), { type: "text_delta", text: delta.text as string });
      }
      if (delta.type === "input_json_delta") petal("tool_input_delta", { partialJson: delta.partial_json as string });
      return;
    }

    if (msg.type === "content_block_stop") {
      if (this.reasoningBlockIdx === msg.index) {
        petal("reasoning_end", { id: msg.index });
        this.reasoningBlockIdx = null;
      }
      petal("content_block_stop", { blockIdx: msg.index });
      return;
    }

    if (msg.type === "assistant" && msg.message) {
      const content = ((msg.message as Record<string, unknown>).content ?? []) as Array<Record<string, unknown>>;
      for (const block of content) {
        if (block.type === "text" && typeof block.text === "string" && !this.streamedTurn) petal("text_delta", { delta: block.text });
        if (block.type === "tool_use") petal("tool_call", { toolCallId: block.id as string, toolName: block.name as string, input: block.input });
      }
      return;
    }

    if (msg.type === "user" && msg.message) {
      const content = ((msg.message as Record<string, unknown>).content ?? []) as Array<Record<string, unknown>>;
      for (const block of content) {
        if (block.type === "tool_result") {
          const toolUseId = (block.tool_use_id as string) ?? "";
          let resultText = "";
          const body = block.content;
          if (typeof body === "string") resultText = body;
          else if (Array.isArray(body)) resultText = (body as Array<Record<string, unknown>>).filter(c => typeof c.text === "string").map(c => c.text as string).join("\n");
          petal("tool_result", { toolUseId, result: resultText });
          this.d.drone.observed(sigil(poolKey), { type: "tool_end" });
        }
      }
      return;
    }

    if (msg.type === "result") {
      this.streamedTurn = false;
      this.d.drone.observed(sigil(poolKey), { type: "turn_end" });
      if (msg.subtype === "error_during_execution" || msg.is_error) {
        trace("stream.result.error", { raw: JSON.stringify(msg).slice(0, 500) });
      }
      listener.onWilt({
        finishReason: (msg.stop_reason as string) ?? "stop",
        usage: msg.usage as Record<string, number> | undefined,
        providerMetadata: { sessionId: msg.session_id, cost: msg.total_cost_usd },
      });
      if (roost.activeSid) {
        roost.listeners.delete(roost.activeSid);
        roost.activeSid = null;
      }
      if (roost.ephemeral && roost.poolKey) {
        trace("nest.turn.end.kept-alive", { poolKey: roost.poolKey, pid: roost.proc.pid });
      }
    }
  }
}

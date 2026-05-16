import { spawn as nodeSpawn, spawnSync as nodeSpawnSync, type ChildProcess } from "node:child_process";
import { createServer as createHttpServer, type IncomingMessage, type ServerResponse } from "node:http";
import { existsSync, unlinkSync, mkdirSync, writeFileSync, readFileSync, readdirSync, statSync, rmSync } from "fs";
import { randomUUID } from "crypto";
import { dirname, join } from "path";
import { fileURLToPath } from "url";

import { trace, info } from "../log.ts";
import { loadConfig } from "../fs/config.ts";
import { sigil, rid as makeRid, echo, pulse, isDusk, WaneTracker, type Tone, type Breath, type BreathSession, type Reach, type PulseKind, type Pulse } from "../thrum/index.ts";
import { Drone, classifySuspicion, droneThink, setDroneWorkspace, releaseDroneSession, stubDrone, Cup, type DroneBeat, type DroneState, type DroneAction } from "../fs/drone/index.ts";
import { graft, createSession as createClaudeSession, sessionDir as getSessionDir, sessionPath as getSessionPath, lastUuid, sanitizeJsonl, pruneJsonl, type GraftResult } from "../fs/session.ts";
import { penny, pennyAdd, pennyLoad, pennySave, pennyReset, type PennyDelta } from "../fs/penny.ts";
import * as drift from "../fs/drift.ts";
import { pickPerch, type RoostProc } from "../nest/index.ts";
import { Nest, type NestDeps } from "../nest/nest.ts";
import type { Roost, BloomListener, PermitHoldEntry } from "../nest/types.ts";
import { mintId } from "../fs/id.ts";


// BloomListener, Roost, encodePrompt/encodeToolResult, parseLine, and the
// Nest class itself now live under /nest. Daemon imports them and wires
// up its own dispatch + permission state as NestDeps below.

function resolveNestName(cwd: string): "claude-repl" | "claude-cli" {
  if (cwd && Array.isArray(cfg.projects)) {
    for (const entry of cfg.projects) {
      if (entry.primaryPath === cwd && entry.nest) return entry.nest;
    }
  }
  return cfg.nest;
}

const cfg = loadConfig();
const MAX_PROCS = cfg.maxProcs;
const IDLE_TIMEOUT = cfg.idleTimeout;


// ─── Permission State ───────────────────────────────────────────────────────

// Pending permission asks — held PreToolUse hook responses waiting for user decision
const HUM_PERMIT_HOLD = new Map<string, {
  resolve: (decision: "allow" | "deny") => void;
  tool: string;
  path?: string;
  sessionId: string;
  createdAt: number;
}>();

function recordPermitHoldSpan(askId: string): void {
  const hold = HUM_PERMIT_HOLD.get(askId);
  if (hold && hold.sessionId) {
    drift.span(hold.sessionId, "permission_hold", Date.now() - hold.createdAt);
  }
}

// Permission rules stored per-session, forwarded from OC via the provider
const sessionPermissions = new Map<string, Array<{ permission: string; pattern: string; action: string }>>();

export function setSessionPermissions(sessionId: string, rules: Array<{ permission: string; pattern: string; action: string }>): void {
  sessionPermissions.set(sessionId, rules);
}

function getPermissionAction(tool: string, path?: string): "allow" | "deny" | "ask" {
  // OC rules are ordered general → specific. Last matching rule wins.
  let result: "allow" | "deny" | "ask" = "allow";
  for (const [, rules] of sessionPermissions) {
    for (const rule of rules) {
      if (rule.permission !== tool && rule.permission !== "*") continue;
      if (path) {
        const pat = rule.pattern;
        if (pat === "*" || path.startsWith(pat.replace("/*", "/")) || path === pat) {
          result = rule.action as "allow" | "deny" | "ask";
        }
      } else if (rule.pattern === "*") {
        result = rule.action as "allow" | "deny" | "ask";
      }
    }
  }
  return result;
}

// ─── Session State (persisted) ───────────────────────────────────────────────

interface Session {
  id: string;
  nest: Array<{ nest: string; id: string }>;
  plugin: Array<{ plugin: string; id: string }>;
  opencodeSessionId: string;
  claudeSessionId: string | null;
  claudeSessionPath: string | null;
  cwd: string;
  modelId: string;
  needsRespawn?: boolean;
  lastAccessed?: number;
  lastSyncedPetal?: string | null; // uuid of last synced JSONL entry
  ocServerUrl?: string;
  thorns?: number; // consecutive error count — circuit breaker
  externalToolNames?: string[]; // sorted names of external MCP tools — respawn on change
  planMode?: boolean; // true when current agent is 'plan' — governs CLI adaptive-thinking env, respawn on change
  // Per-session cached thrum fields. Plugin dedups these — when a thrum
  // message omits them, we fall back to the cached value so a cold-spawn
  // still gets the right system prompt / permissions / allowedTools.
  lastSystemPrompt?: string;
  lastPermissions?: unknown[];
  lastAllowedTools?: string[];
  // Largest per-turn input context (input + cache_create + cache_read) seen
  // so far, captured from each result event's usage block. Pure observation:
  // surfaced via `hum savings` and used to emit a warning trace when a
  // session climbs past CONTEXT_WARN_THRESHOLD. Context reduction is OC's
  // job — hum does not mutate session state on this signal.
  maxContextTokens?: number;
  // Callable closure: forces cupped petals to be sent immediately.
  // Set by the onPetal IIFE in the prompt handler. The tendril callback
  // calls this before sending tendril-reach so the provider has tool
  // events in its buds when the hold signal arrives.
  forceUncup?: () => void;
}

// Advisory threshold. When a session's peak per-turn input context crosses
// this value, hum emits a `context.over.threshold.warning` trace on the
// next prompt and bumps `penny.contextOverThreshold`. Operator-facing signal
// only — no state mutation.
const CONTEXT_WARN_THRESHOLD = Number(process.env.HUM_CONTEXT_WARN ?? "300000");

const STATE_DIR = process.env.XDG_STATE_HOME
  ? `${process.env.XDG_STATE_HOME}/hum`
  : `${process.env.HOME}/.local/state/hum`;
const SESSIONS_FILE = `${STATE_DIR}/sessions.json`;
const PENNY_FILE = `${STATE_DIR}/penny.json`;

// Load persisted penny counters (lifetime view) and start a write-back timer.
pennyLoad(PENNY_FILE);
const pennyPersistInterval = setInterval(() => pennySave(PENNY_FILE), 60_000);
pennyPersistInterval.unref?.();
process.on("SIGTERM", () => { try { pennySave(PENNY_FILE); } catch {} });
process.on("SIGINT", () => { try { pennySave(PENNY_FILE); } catch {} });

function loadSessions(): Map<string, Session> {
  try {
    const data = JSON.parse(readFileSync(SESSIONS_FILE, "utf-8"));
    const map = new Map<string, Session>(Object.entries(data) as Array<[string, Session]>);
    let idBack = 0, nestBack = 0, pluginBack = 0;
    for (const [oid, s] of map) {
      if (!s.id || typeof s.id !== "string") { s.id = mintId(); idBack++; }
      if (!Array.isArray(s.nest)) {
        s.nest = [{ nest: resolveNestName(s.cwd), id: s.claudeSessionId ?? "" }];
        nestBack++;
      }
      if (!Array.isArray(s.plugin)) {
        s.plugin = [{ plugin: "opencode", id: s.opencodeSessionId ?? oid }];
        pluginBack++;
      }
    }
    if (idBack || nestBack || pluginBack) trace("session.backfilled", { id: idBack, nest: nestBack, plugin: pluginBack });
    return map;
  } catch {
    return new Map();
  }
}

function saveSessions(mutatedSid?: string): void {
  if (mutatedSid) {
    const s = sigil(mutatedSid);
    const w = wane.tick(s);
    drone.setWane(s, w);
  }
  try {
    mkdirSync(STATE_DIR, { recursive: true });
    const obj: Record<string, Session> = {};
    for (const [k, v] of sessions) obj[k] = v;
    writeFileSync(SESSIONS_FILE, JSON.stringify(obj));
  } catch {}
}

const sessions = loadSessions();

// ─── HTTP Server ─────────────────────────────────────────────────────────────

function defaultSocketPath(): string {
  const runtime = process.env.XDG_RUNTIME_DIR;
  if (runtime) {
    const dir = `${runtime}/hum`;
    mkdirSync(dir, { recursive: true });
    return `${dir}/hum.sock`;
  }
  // macOS / linux without XDG_RUNTIME_DIR — UID-namespaced /tmp dir
  // (kept in sync with plugin's defaultSocketPath in provider.ts).
  const uid = process.getuid?.() ?? 0;
  const dir = `/tmp/hum-${uid}`;
  mkdirSync(dir, { recursive: true });
  return `${dir}/hum.sock`;
}


const SOCK = process.env.HUM_SOCKET ?? defaultSocketPath();
const HTTP = SOCK + ".http";


// Nest is constructed at the bottom of the file once its deps (thrum,
// drone, drift, permission helpers) are defined; declared up here so
// the rest of daemon can reference `nest.<method>` without dep ordering
// gymnastics.
let nest!: Nest;
const wane = new WaneTracker();
const DEFAULT_OC_URL = "http://127.0.0.1:4096";

// Drone: self-governing observer — opt-in via droned:true in hum.json
// When off, the thrum is a raw pipe. When on, the sentinel watches everything.
const DRONED = cfg.droned;

function ocUrlForSigil(s: string): string {
  for (const [sid, session] of sessions) {
    if (sigil(sid) === s) return session.ocServerUrl ?? DEFAULT_OC_URL;
  }
  return DEFAULT_OC_URL;
}

function droneCtx(text: string, state: DroneState): Parameters<typeof droneThink>[0] {
  return {
    responseText: text,
    inflightTools: state.inflightTools,
    pendingPermissions: state.pendingPermissions,
    tokensBurned: state.tokensBurned,
    turnCount: 0,
    localWane: state.localWane,
    remoteWane: state.remoteWane,
    missedBeats: state.missedBeats,
    pendingEchoes: state.pendingEchoes.size,
    toolNames: [],
  };
}

const droneEvaluator = async (text: string, state: DroneState): Promise<number> => {
  const level = classifySuspicion(text);

  // Clean: no heuristic flags
  if (level === "none") return 0.1;

  // Critical: near-certain context loss — high confidence without LLM
  if (level === "critical") {
    trace("drone.heuristic.critical", { sigil: state.sigil, text: text.slice(0, 200) });
    return 0.9;
  }

  // Suspicious: compensation detected — ask the LLM to confirm
  trace("drone.heuristic.suspicious", { sigil: state.sigil, text: text.slice(0, 200) });
  try {
    const s = state.sigil;
    const url = s ? ocUrlForSigil(s) : DEFAULT_OC_URL;
    const judgment = await droneThink(droneCtx(text, state), url, s);
    trace("drone.llm.judgment", { assessment: judgment.assessment, action: judgment.action, reason: judgment.reason });
    return judgment.action === "swallow" ? 0.9 : judgment.action === "none" ? 0.2 : 0.6;
  } catch (e) {
    trace("drone.llm.failed", { err: String(e) });
    // Suspicious + LLM unreachable — lean towards swallow
    return 0.75;
  }
};

const drone = DRONED ? new Drone("daemon", (action: DroneAction) => {
  switch (action.type) {
    case "beat":
      // Send drone beat to the sigil's client
      for (const [, client] of thrumClients) {
        if (client.sigils.has(action.sigil) || client.sigils.size === 0) {
          thrumTo(client, action.beat as unknown as Record<string, unknown>);
          break;
        }
      }
      break;
    case "retry":
      trace("drone.retry", { sigil: action.sigil, rid: action.rid, chi: action.chi });
      // Retry is handled by re-acking — the original tone was already processed,
      // the echo was lost. Re-send echo.
      for (const [, client] of thrumClients) {
        if (client.sigils.has(action.sigil)) {
          thrumTo(client, { chi: "echo", rid: action.rid, ok: true, retried: true });
          break;
        }
      }
      break;
    case "lost":
      trace("drone.lost", { sigil: action.sigil, rid: action.rid, chi: action.chi });
      break;
    case "drift":
      trace("drone.drift", { sigil: action.sigil, local: action.local, remote: action.remote });
      // Trigger breath resync to the drifted client
      for (const [, client] of thrumClients) {
        if (client.sigils.has(action.sigil)) {
          thrumBreath(client);
          break;
        }
      }
      break;
    case "dead":
      trace("drone.dead", { sigil: action.sigil, missedBeats: action.missedBeats });
      // Clean up stale sigil — but don't disconnect the client (it may own active sessions too)
      for (const [, client] of thrumClients) {
        if (client.sigils.has(action.sigil)) {
          client.sigils.delete(action.sigil);
          break;
        }
      }
      break;
    case "swallow":
      // Wither is handled by cupped petals in onPetal — daemon-side only.
      // The Drone class may still emit this from its evaluator, but onPetal already acted.
      trace("drone.wither.noop", { sigil: action.sigil, reason: action.reason });
      break;
  }
}, droneEvaluator, 0.7, (s: string, state: DroneState) => {
  // LLM assessment on silence — full state evaluation
  droneThink(droneCtx(state.responseText, state), ocUrlForSigil(s), s).then(judgment => {
    trace("drone.llm.assess", { sigil: s, assessment: judgment.assessment, action: judgment.action, reason: judgment.reason });
    // Act on LLM judgment
    if (judgment.action === "respawn") {
      for (const [sid, session] of sessions) {
        if (sigil(sid) === s) { nest.fell(sid, sid); break; }
      }
    } else if (judgment.action === "reseed") {
      for (const [sid, session] of sessions) {
        if (sigil(sid) === s) { session.needsRespawn = true; saveSessions(sid); break; }
      }
    } else if (judgment.action === "swallow") {
      // Swallow from silence path — by the time silence fires, the stream is done.
      // Set needsRespawn so next prompt starts fresh.
      for (const [sid, session] of sessions) {
        if (sigil(sid) === s) {
          session.needsRespawn = true;
          saveSessions(sid);
          break;
        }
      }
    }
  }).catch(e => trace("drone.llm.assess.failed", { sigil: s, err: String(e) }));
}) : stubDrone();

const HUM = SOCK + ".thrum";

for (const p of [SOCK, HTTP, HUM]) {
  mkdirSync(dirname(p), { recursive: true });
  if (existsSync(p)) { try { unlinkSync(p); } catch {} }
}

// ─── humHum: Bidirectional NDJSON socket ─────────────────────────────────
// One persistent connection per provider instance. Both sides push typed
// JSON messages (chi = message type). Replaces HTTP request/response dance.

const thrumClients = new Map<string, Reach>();

function thrumTo(client: Reach, msg: Record<string, unknown> | Breath): void {
  try { client.socket.write(JSON.stringify(msg) + "\n"); } catch {}
}

function thrumAll(msg: Record<string, unknown>): void {
  trace("thrum.all", { chi: msg.chi as string, clients: thrumClients.size });
  for (const [, client] of thrumClients) thrumTo(client, msg);
}

function thrum(sessionId: string, msg: Record<string, unknown>): void {
  const s = sigil(sessionId);
  if (!msg.rid) msg.rid = makeRid();
  if (!msg.sigil) msg.sigil = s;
  if (!msg.sid) msg.sid = sessionId;
  msg.from = "daemon";
  trace("thrum.tone.sent", { chi: msg.chi as string, sid: sessionId, rid: msg.rid as string });
  // Route to first client that owns this sigil — no duplicates
  let sent = false;
  for (const [, client] of thrumClients) {
    if (client.sigils.has(s)) {
      thrumTo(client, msg);
      sent = true;
      break;
    }
  }
  // Fallback: if no client claimed this sigil, broadcast to unregistered clients
  if (!sent) {
    for (const [, client] of thrumClients) {
      if (client.sigils.size === 0) thrumTo(client, msg);
    }
  }
  // Drone observes outgoing tones
  drone.sent(msg);
}

function thrumBreath(client: Reach): void {
  const sessionList: BreathSession[] = [];
  for (const [sid, session] of sessions) {
    // Only sync sessions with meaningful state
    if (!session.claudeSessionId && !session.lastSyncedPetal) continue;
    const s = sigil(sid);
    sessionList.push({
      sigil: s,
      sid,
      claudeSessionId: session.claudeSessionId,
      claudeSessionPath: session.claudeSessionPath,
      lastSyncedPetal: session.lastSyncedPetal ?? null,
      wane: wane.get(s),
      modelId: session.modelId,
      cwd: session.cwd,
      roostAlive: !!nest.roost(sid),
      roostPid: nest.roost(sid)?.proc.pid,
    });
  }
  const msg: Breath = { chi: "breath", from: "daemon", sessions: sessionList };
  thrumTo(client, msg);
  trace("thrum.breath.sent", { clientId: client.clientId.slice(0, 8), sessions: sessionList.length });
}

function thrumEcho(clientId: string, tone: Record<string, unknown>, ok = true, error?: string): void {
  const client = thrumClients.get(clientId);
  if (!client) return;
  thrumTo(client, { chi: "echo", rid: tone.rid, ok, error });
}

function thrumPulse(kind: PulseKind, sid: string, extra?: Partial<Pulse>): void {
  const p = pulse(kind, sigil(sid), sid, extra);
  thrum(sid, p as unknown as Record<string, unknown>);
}

function thrumHear(clientId: string, msg: Record<string, unknown>): void {
  const chi = msg.chi as string;
  if (chi !== "log") trace("thrum.tone.received", { chi, clientId: clientId.slice(0, 8), rid: msg.rid as string });

  // Drone observes incoming tones
  drone.heard(msg);

  // Dusk: discard tones past their expiry
  if (isDusk(msg)) {
    trace("thrum.tone.dusk", { chi, rid: msg.rid as string, dusk: msg.dusk });
    thrumEcho(clientId, msg, false, "past dusk");
    return;
  }

  // Echo: acknowledge receipt
  if (chi !== "echo" && chi !== "log" && msg.rid) {
    thrumEcho(clientId, msg);
  }

  // Per-thrum transit timing — plugin stamps `sentAt` on outgoing tones,
  // daemon records the time-to-receive as one thrum sample. Direction is
  // anchored on the destination endpoint (`to: "nest"` = arriving at the
  // daemon's nest). Aggregates as p50/p95 across all sampled hums per
  // bloom; not cumulative.
  if (typeof msg.sentAt === "number" && typeof msg.sid === "string") {
    drift.thrum(msg.sid as string, "nest", Date.now() - (msg.sentAt as number));
  }

  switch (chi) {
    case "perf-mark": {
      // Plugin-emitted perf marks. Daemon merges into the active bloom's
      // record. Each field is optional; multiple may be set per message.
      const sid = msg.sid as string;
      const mark = msg.mark as string | undefined;
      const span = msg.span as { name: string; ms: number } | undefined;
      const flag = msg.flag as { key: string; value: boolean | number | string } | undefined;
      const thrumSample = msg.thrum as { to: "nest" | "oc"; ms: number } | undefined;
      if (mark) drift.mark(sid, mark);
      if (span) drift.span(sid, span.name, span.ms);
      if (flag) drift.flag(sid, flag.key, flag.value);
      if (thrumSample && (thrumSample.to === "nest" || thrumSample.to === "oc")) {
        drift.thrum(sid, thrumSample.to, thrumSample.ms);
      }
      break;
    }
    case "prompt": {
      // Plugin piggybacks its counter delta on every prompt — ingest before
      // anything else so counts don't miss on errors/early returns below.
      if (msg.pennyDelta) pennyAdd(msg.pennyDelta as PennyDelta);

      const sid = msg.sid as string;
      drift.open(sid, msg.modelId as string | undefined);
      const client = thrumClients.get(clientId);
      (async () => {
      if (client) client.sigils.add(sigil(sid));

      // Get or create session
      let session = sessions.get(sid);
      if (!session) {
        const cwd = (msg.cwd as string) ?? "/root";
        session = {
          id: mintId(),
          nest: [{ nest: resolveNestName(cwd), id: "" }],
          plugin: [{ plugin: "opencode", id: sid }],
          opencodeSessionId: sid,
          claudeSessionId: null,
          claudeSessionPath: null,
          cwd,
          modelId: (msg.modelId as string) ?? "sonnet",
        };
        sessions.set(sid, session);
        saveSessions(sid);
        trace("session.created", { sid, model: session.modelId });
      }
      session.lastAccessed = Date.now();

      // Plugin may omit these fields on steady-state turns (hash-dedup). Fall
      // back to the session's last-known value so cold-spawns and respawns
      // still get the correct system prompt / permissions / allowedTools.
      const permissions = ("permissions" in msg
        ? (msg.permissions as unknown[] ?? [])
        : (session.lastPermissions ?? [])) as unknown[];
      const systemPrompt = ("systemPrompt" in msg
        ? ((msg.systemPrompt as string) || undefined)
        : session.lastSystemPrompt);
      const allowedTools = ("allowedTools" in msg
        ? ((msg.allowedTools as string[]) || undefined)
        : session.lastAllowedTools);
      // Cache fresh values when the plugin sent them.
      if ("permissions" in msg) session.lastPermissions = permissions;
      if ("systemPrompt" in msg && systemPrompt !== undefined) session.lastSystemPrompt = systemPrompt;
      if ("allowedTools" in msg && allowedTools !== undefined) session.lastAllowedTools = allowedTools;
      const cwd = msg.cwd as string | undefined;
      const ocServerUrl = (msg.ocServerUrl as string) || DEFAULT_OC_URL;

      if (cwd) mcpSetCwd(cwd);
      if (permissions.length > 0) {
        setSessionPermissions(sid, permissions as any);
      }

      const poolKey = sid;

      // Update model, cwd, and OC server URL — prompt always carries current values
      if (msg.modelId) session.modelId = msg.modelId as string;
      if (cwd) session.cwd = cwd;
      if (ocServerUrl !== DEFAULT_OC_URL) session.ocServerUrl = ocServerUrl;

      // Skip tool registration on listenOnly (permission return) — avoid spurious respawn
      if (!msg.listenOnly) {
        // Plan-mode toggle — respawn when it changes so the spawn env
        // picks up the right CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING value.
        const nextPlan = !!msg.planMode;
        if (nextPlan !== !!session.planMode) {
          session.planMode = nextPlan;
          session.needsRespawn = true;
          trace("plan.mode.changed", { sid, planMode: nextPlan });
        }

        // External MCP tools — register for this session, respawn if changed
        const extTools = (msg.externalTools as ExternalToolDef[] | undefined) ?? [];
        const prevNames = (session.externalToolNames ?? []).join(",");
        const currNames = extTools.map(t => t.name).sort().join(",");
        if (extTools.length > 0) setExternalTools(sid, extTools);
        else clearExternalTools(sid);
        if (currNames !== prevNames) {
          session.externalToolNames = extTools.map(t => t.name).sort();
          if (prevNames) {
            session.needsRespawn = true;
            trace("external.tools.changed", { sid, prev: prevNames, curr: currNames });
          } else if (extTools.length > 0) {
            trace("external.tools.registered", { sid, count: extTools.length, names: currNames });
          }
        }

        // External MCP server configs — daemon connects directly for tool execution
        const mcpConfigs = (msg.mcpServerConfigs as Array<Record<string, any>> | undefined) ?? [];
        if (mcpConfigs.length > 0) {
          setMcpServerConfigs(sid, mcpConfigs as any);
          trace("mcp.configs.registered", { sid, servers: mcpConfigs.map(c => c.name).join(",") });
        } else {
          clearMcpServerConfigs(sid);
        }

        // Visible tools — OC decides what Claude sees (filtered by agent permissions)
        const visibleToolNames = msg.visibleTools as string[] | undefined;
        if (visibleToolNames && visibleToolNames.length > 0) {
          setVisibleTools(sid, visibleToolNames);
        }
      }

      // Circuit breaker — stop after 3 consecutive errors
      const MAX_THORNS = 3;
      if ((session.thorns ?? 0) >= MAX_THORNS) {
        trace("nest.thorns.breaker", { sid, thorns: session.thorns });
        thrum(sid, { chi: "error", sid, message: `circuit breaker: ${session.thorns} consecutive errors` });
        return;
      }

      // Advisory: if the prior turn's peak input context crossed the warning
      // threshold, emit a trace and bump the penny counter so an operator
      // sees a session climbing toward the cache-replay tax. No state
      // mutation; OC owns context reduction.
      if (!msg.listenOnly && (session.maxContextTokens ?? 0) > CONTEXT_WARN_THRESHOLD) {
        penny.contextOverThreshold++;
        trace("context.over.threshold.warning", {
          sid,
          maxCtx: session.maxContextTokens,
          threshold: CONTEXT_WARN_THRESHOLD,
        });
      }

      // Graft: sync OC petals into Claude JSONL before spawning (skip for title gen / empty tools)
      const priorPetals = msg.priorPetals as Array<{ role: string; content: unknown }> | undefined;
      if (!msg.listenOnly && !msg.skipGraft && priorPetals && priorPetals.length > 0) {
        trace("graft.enter", { sid, petals: priorPetals.length });
        const graftStart = Date.now();
        try {
          const effectiveCwd = cwd ?? session.cwd;
          if (session.claudeSessionId && session.claudeSessionPath) {
            // Existing JSONL — graft any new petals
            const result = graft(priorPetals ?? [], session.claudeSessionPath, session.claudeSessionId, effectiveCwd, session.lastSyncedPetal);
            // Always update anchor — even grafted=0, the JSONL may have grown from Claude's native entries
            if (result.lastPetal) session.lastSyncedPetal = result.lastPetal;
            if (result.grafted > 0) {
              session.needsRespawn = true;
              saveSessions(sid);
              trace("graft.done", { sid, grafted: result.grafted });
            }
          } else {
            // Cold start — peek OC for petals, create JSONL only if there's content
            const peekId = randomUUID();
            const peekPath = createClaudeSession(effectiveCwd, peekId);
            const result = graft(priorPetals ?? [], peekPath, peekId, effectiveCwd);
            if (result.grafted > 0) {
              session.claudeSessionId = peekId;
              if (session.nest[0]) session.nest[0].id = peekId;
              session.claudeSessionPath = peekPath;
              session.lastSyncedPetal = result.lastPetal;
              session.needsRespawn = true;
              saveSessions(sid);
              trace("graft.cold", { sid, grafted: result.grafted });
            } else {
              // No petals — delete the empty JSONL skeleton
              trace("graft.cold.empty", { sid });
              try { unlinkSync(peekPath); } catch {}
            }
          }
        } catch (e) {
          trace("graft.failed", { sid, err: String(e) });
        }
        drift.span(sid, "graft", Date.now() - graftStart);
        drift.mark(sid, "graft_synced");
      }

      // Capture prompt content for deferred murmur
      const promptContent: Array<Record<string, unknown>> | string | null =
        !msg.listenOnly ? (msg.content as Array<Record<string, unknown>> | undefined) ?? (msg.text as string ?? "") : null;
      const isResume = !!(session.claudeSessionId && session.needsRespawn);
      let cup: Cup | null = null; // owns the drone's cup buffer; assigned in onPetal closure
      let uncup: (() => void) | null = null; // closure-shim onto cup.forceFlush — called from onWilt

      const listener: BloomListener = {
        sessionId: sid,
        onRoost(claudeSessionId, model, tools) {
          session.claudeSessionId = claudeSessionId;
          if (session.nest[0]) session.nest[0].id = claudeSessionId;
          if (!session.claudeSessionPath) {
            // Use the cwd passed to awaken (= spawnCwd), not session.cwd,
            // because Claude CLI determines its project dir from spawnCwd
            const effectiveCwd = cwd ?? session.cwd;
            const dir = getSessionDir(effectiveCwd);
            try { mkdirSync(dir, { recursive: true }); } catch {}
            session.claudeSessionPath = getSessionPath(effectiveCwd, claudeSessionId);
          }
          saveSessions(sid);
          thrum(sid, { chi: "session-ready", sid, claudeSessionId, model, tools });
          thrumPulse("roost-ready", sid, { pid: nest.roost(poolKey)?.proc.pid });
        },
        onPetal: (() => {
          // Outbound socket batching: independent of the drone's cup. The cup
          // decides WHEN to release; this microtask coalesces socket writes.
          let batch: string[] = [];
          let pending = false;

          function sendChunks(chunks: string[]) {
            drift.mark(sid, "first_bloom");
            // Stamp sentAt on the first chunk only, at the actual send time
            // (not at petal creation, which can be cup-delayed by seconds).
            // Plugin only reads sentAt on first chunk per turn for hop timing.
            if (chunks.length > 0) {
              chunks = [chunks[0].replace(/}$/, `,"sentAt":${Date.now()}}`), ...chunks.slice(1)];
            }
            const line = chunks.join("\n") + "\n";
            const s = sigil(sid);
            let sent = false;
            for (const [, client] of thrumClients) {
              if (client.sigils.has(s)) {
                try { client.socket.write(line); } catch {}
                sent = true;
                break;
              }
            }
            if (!sent) {
              for (const [, client] of thrumClients) {
                if (client.sigils.size === 0) try { client.socket.write(line); } catch {}
              }
            }
          }

          cup = new Cup(
            { enabled: DRONED },
            {
              onBloom: (chunks) => {
                drift.mark(sid, "first_uncup");
                sendChunks(chunks);
              },
              onApiError: (text) => {
                trace("nest.api.error", { sid, text: text.slice(0, 120) });
                thrum(sid, { chi: "error", sid, message: text });
                nest.interrupt(poolKey);
              },
              onWither: () => {
                if (!session) return;
                trace("drone.wither", { sid });
                penny.droneWithers++;
                nest.fell(sid, poolKey);
                session.needsRespawn = true;
                session.lastSyncedPetal = null;
                saveSessions(sid);
                // Drift: keep startedAt, clear marks so the retry's first_petal etc.
                // record fresh values; flags.withered increments to track retries.
                drift.witherReset(sid);
                // Re-send the prompt after a tick (let fell complete)
                queueMicrotask(() => {
                  cup?.reset();
                  batch = [];
                  pending = false;
                  if (promptContent) {
                    (async () => {
                      try {
                        session.needsRespawn = true;
                        await nest.awaken(poolKey, session.modelId, listener, session.claudeSessionId ?? undefined, permissions, systemPrompt, allowedTools, cwd, session.planMode);
                        nest.murmur(sid, poolKey, promptContent);
                      } catch (e) {
                        trace("drone.swallow.retry.failed", { sid, err: String(e) });
                        thrum(sid, { chi: "error", sid, message: `swallow retry failed: ${e}` });
                      }
                    })();
                  }
                });
              },
              onTrace: (ev, data) => trace(ev, { sid, ...data }),
            },
          );

          uncup = () => cup?.forceFlush();
          if (session) session.forceUncup = uncup;

          // Per-tool-call arg-stream timer + reasoning duration tracker.
          // Drift accounts for "input thinking" (tool_input_start → tool_call)
          // and reasoning span (reasoning_start → reasoning_end). Closure-local.
          const toolArgStarts = new Map<string, number>();
          let reasoningStartedAt = 0;

          return (type: string, payload: Record<string, unknown>) => {
            if (!cup || cup.withered) return;
            // first_petal = first data-bearing petal from Claude CLI's stream.
            // Skip housekeeping types (stream_start fires at nest spawn,
            // before any content) so the murmur→first_petal gap reflects
            // real TTFB to first content, not synthetic bookkeeping.
            if (type !== "stream_start" && type !== "content_block_stop") {
              drift.mark(sid, "first_petal");
            }

            // Per-block-type first-time marks + per-call tracking
            if (type === "reasoning_start") {
              drift.mark(sid, "first_reasoning_start");
              reasoningStartedAt = Date.now();
            } else if (type === "reasoning_end" && reasoningStartedAt) {
              drift.span(sid, "reasoning", Date.now() - reasoningStartedAt);
              reasoningStartedAt = 0;
            } else if (type === "text_start") {
              drift.mark(sid, "first_text_start");
            } else if (type === "tool_input_start" && payload.toolCallId) {
              drift.mark(sid, "first_tool_input_start");
              toolArgStarts.set(payload.toolCallId as string, Date.now());
            } else if (type === "tool_call" && payload.toolCallId) {
              const callId = payload.toolCallId as string;
              const startedAt = toolArgStarts.get(callId);
              if (startedAt) {
                const toolName = (payload.toolName as string) ?? "unknown";
                drift.span(sid, `tool_args:${toolName}`, Date.now() - startedAt);
                toolArgStarts.delete(callId);
              }
            }

            const chunk = JSON.stringify({ chi: "chunk", sid, chunkType: type, ...payload });

            const verdict = cup.feed(type, payload, chunk);
            if (verdict === "withered" || verdict === "buffered") return;

            // passthrough (uncupped) — microtask-batch + send
            batch.push(chunk);
            if (!pending) {
              pending = true;
              queueMicrotask(() => {
                sendChunks(batch);
                batch = [];
                pending = false;
              });
            }
          };
        })(),
        onWilt(harvest) {
          if (cup?.withered) return; // withered petal — don't send finish for bad petals
          session.thorns = 0; // reset circuit breaker on success
          // Advance anchor to last JSONL entry — Claude finished writing
          if (session.claudeSessionPath) {
            const tip = lastUuid(session.claudeSessionPath);
            if (tip) session.lastSyncedPetal = tip;
          }
          // Track peak per-turn input context for observability — surfaced
          // by `hum savings` and used by the next-prompt warning trace.
          // No destructive action attached: hum does not rotate sessions.
          penny.blooms++;
          if (harvest.usage) {
            const u = harvest.usage;
            const turnCtx = (u.input_tokens ?? 0) + (u.cache_creation_input_tokens ?? 0) + (u.cache_read_input_tokens ?? 0);
            if (turnCtx > (session.maxContextTokens ?? 0)) {
              session.maxContextTokens = turnCtx;
            }
            penny.totalInputTokens += (u.input_tokens ?? 0);
            penny.totalOutputTokens += (u.output_tokens ?? 0);
            penny.totalCacheReadTokens += (u.cache_read_input_tokens ?? 0);
            penny.totalCacheWriteTokens += (u.cache_creation_input_tokens ?? 0);
          }
          if (harvest.providerMetadata) {
            const cost = (harvest.providerMetadata as any).cost;
            if (typeof cost === "number") penny.totalCost += cost;
          }
          trace("nest.wilt", { sid, finishReason: harvest.finishReason, maxCtx: session.maxContextTokens });
          if (uncup) uncup(); // uncup any remaining petals before finish
          thrum(sid, {
            chi: "finish", sid,
            finishReason: harvest.finishReason,
            usage: harvest.usage,
            providerMetadata: harvest.providerMetadata,
          });
          drift.wilt(sid);
          nest.hush(sid, poolKey);
        },
        onThorn(wound) {
          session.thorns = (session.thorns ?? 0) + 1;
          trace("nest.thorn", { sid, wound: wound.slice(0, 100), thorns: session.thorns });
          thrum(sid, { chi: "error", sid, message: wound });
          nest.fell(sid, poolKey);
        },
      };

      const hadRoost = !!nest.roost(poolKey);
      const awakenStart = Date.now();
      await nest.awaken(poolKey, session.modelId, listener, session.claudeSessionId ?? undefined, permissions, systemPrompt, allowedTools, cwd, session.planMode);
      if (hadRoost) {
        drift.flag(sid, "warm", true);
      } else {
        drift.span(sid, "nest_spawn", Date.now() - awakenStart);
        drift.mark(sid, "nest_spawned");
      }

      if (promptContent) {
        // Guard against empty murmurs — empty text blocks cause API 400 (cache_control on empty text)
        const hasContent = typeof promptContent === "string"
          ? promptContent.length > 0
          : Array.isArray(promptContent) && promptContent.some((p: Record<string, unknown>) => p.type !== "text" || (p.text as string)?.length > 0);
        if (hasContent) {
          drift.mark(sid, "murmur");
          nest.murmur(sid, poolKey, promptContent);
        } else {
          trace("nest.murmur.empty", { sid, poolKey });
          // Send finish so OC doesn't hang waiting for a response
          thrum(sid, { chi: "finish", sid, finishReason: "stop", usage: undefined, providerMetadata: {} });
        }
      }
      })().catch(e => trace("prompt.failed", { sid, err: String(e) }));
      break;
    }

    case "cancel": {
      const sid = msg.sid as string;
      const session = sessions.get(sid);
      if (session) {
        trace("nest.cancelled", { sid, reason: msg.reason });
        if (msg.reason === "compaction") {
          // Compaction: kill the running Claude CLI process and TRUNCATE the
          // existing JSONL in place. The claudeSessionId / claudeSessionPath
          // STAY THE SAME — hum's invariant is one-OC-session-to-one-Claude-
          // session, stable for the lifetime of the OC session. Next prompt
          // takes the warm-path graft, which sees an effectively-empty JSONL
          // (just the summary header) and writes the compacted history into
          // it. Claude resumes from the same uuid with fresh content.
          nest.fell(sid, sid);
          if (session.claudeSessionPath && session.claudeSessionId) {
            try {
              createClaudeSession(session.cwd, session.claudeSessionId);
              trace("compaction.jsonl.truncated", { sid, path: session.claudeSessionPath });
            } catch (e) {
              trace("compaction.truncate.failed", { sid, err: String(e) });
            }
          }
          session.lastSyncedPetal = null;
          session.needsRespawn = true;
          saveSessions(sid);
        } else if (msg.reason === "swallow") {
          // Drone swallow: kill process — plugin re-sends prompt, daemon re-seeds via graft
          nest.fell(sid, sid);
          session.needsRespawn = true;
          session.lastSyncedPetal = null;
          saveSessions(sid);
          thrum(sid, { chi: "drone-retrofit", sid, reason: msg.reason });
        } else {
          // User interrupt: stop current generation immediately via
          // control_cancel_request, AND mark the session for respawn on the
          // next prompt. Claude CLI in -p mode after a cancel does not
          // reliably reconsume stdin — follow-up user messages got silently
          // swallowed. Respawning on next prompt guarantees the new message
          // lands on a fresh process; sanitizeJsonl strips the interrupted
          // trailing-tool_use on --resume, so context stays coherent.
          nest.interrupt(sid);
          session.needsRespawn = true;
          session.lastSyncedPetal = null;
          saveSessions(sid);
        }
      }
      break;
    }

    case "curate": {
      // Provider intercepted OC compaction — prune the JSONL instead of
      // letting OC summarize. Kill the process, prune, respawn on next prompt.
      const sid = msg.sid as string;
      const session = sessions.get(sid);
      if (session) {
        nest.fell(sid, sid);
        if (session.claudeSessionPath) {
          const pruneStart = Date.now();
          try {
            const result = pruneJsonl(session.claudeSessionPath);
            drift.span(sid, "compaction_curate", Date.now() - pruneStart);
            penny.curateEvents++;
            penny.curateBytesSaved += result.bytes.before - result.bytes.after;
            penny.curateThinkingStripped += result.stripped;
            trace("curate.pruned", {
              sid,
              trimmed: result.trimmed,
              stripped: result.stripped,
              before: result.bytes.before,
              after: result.bytes.after,
              saved: result.bytes.before - result.bytes.after,
            });
          } catch (e) {
            trace("curate.failed", { sid, err: String(e) });
          }
        }
        session.lastSyncedPetal = null;
        session.needsRespawn = true;
        saveSessions(sid);
      }
      break;
    }

    case "tendril-result": {
      const callId = msg.callId as string;
      const result = msg.result as string;
      trace("tendril.result", { callId, len: result?.length });
      resolveTendril(callId, result ?? "");
      break;
    }

    case "release-permit": {
      const askId = msg.askId as string;
      const decision = msg.decision as "allow" | "deny";
      const hold = HUM_PERMIT_HOLD.get(askId);
      trace("thrum.permit.releasing", { askId, decision, holdExists: !!hold, pendingHolds: HUM_PERMIT_HOLD.size });
      if (hold) {
        recordPermitHoldSpan(askId);
        HUM_PERMIT_HOLD.delete(askId);
        hold.resolve(decision);
        trace("thrum.permit.released", { askId, decision });
        // Drone observes permission resolution — find the session
        const permitSid = hold.sessionId;
        if (permitSid) drone.observed(sigil(permitSid), { type: "permission_resolved" });
      }
      break;
    }

    case "cleanup": {
      const sid = msg.sid as string;
      const session = sessions.get(sid);
      if (session) {
        nest.fell(sid, sid);
        releaseDroneSession(sigil(sid));
        clearExternalTools(sid);
        clearMcpServerConfigs(sid);
        clearVisibleTools(sid);
        clearReadCache(sid);
        sessions.delete(sid);
        saveSessions(sid);
        trace("thrum.session.cleaned", { sid });
      }
      break;
    }

    // "seeded" handler removed — daemon owns seeding via graft()

    case "log": {
      const level = msg.level as string;
      const event = msg.event as string;
      const data = msg.data as Record<string, unknown> | undefined;
      if (level === "info") info(event, data);
      else trace(event, data);
      break;
    }

    case "petal-cell": {
      // The sentinel's ears — track all OC session activity across all providers
      const sid = msg.sid as string;
      const role = msg.role as string;
      const model = msg.model as string;
      const provider = msg.provider as string;
      const messageId = msg.messageId as string | undefined;
      const parentId = msg.parentId as string | undefined;
      const completed = msg.completed as number | undefined;
      trace("petal.cell", { sid, role, provider, messageId, completed: !!completed });
      let session = sessions.get(sid);
      if (!session) {
        const cwd = process.env.HOME ?? "/";
        session = {
          id: mintId(),
          nest: [{ nest: resolveNestName(cwd), id: "" }],
          plugin: [{ plugin: "opencode", id: sid }],
          opencodeSessionId: sid,
          claudeSessionId: null,
          claudeSessionPath: null,
          cwd,
          modelId: model ?? "sonnet",
        };
        sessions.set(sid, session);
      }
      session.lastAccessed = Date.now();
      // Advance anchor on completed hum assistant petals —
      // completed means Claude finished writing to the JSONL, read the last uuid
      if (role === "assistant" && completed && provider?.startsWith("opencode-hum") && session.claudeSessionPath) {
        const tip = lastUuid(session.claudeSessionPath);
        if (tip) {
          session.lastSyncedPetal = tip;
          saveSessions(sid);
        }
      }
      break;
    }

    case "drone":
      // Plugin drone beat — handled by drone.heard() above
      break;

    default:
      trace("thrum.msg.unknown", { chi });
  }
}

import { createServer, type Socket } from "net";

const thrumServer = createServer((socket: Socket) => {
  const clientId = randomUUID();
  const client: Reach = { clientId, sigils: new Set(), socket };
  thrumClients.set(clientId, client);
  info("thrum.connected", { clientId: clientId.slice(0, 8), total: thrumClients.size });

  // Breath: send full state on connect
  thrumBreath(client);

  let buf = "";
  socket.on("data", (data) => {
    buf += data.toString();
    const lines = buf.split("\n");
    buf = lines.pop() ?? "";
    for (const line of lines) {
      if (!line) continue;
      try {
        thrumHear(clientId, JSON.parse(line));
      } catch (e) {
        trace("thrum.parse.failed", { err: String(e) });
      }
    }
  });

  socket.on("close", () => {
    thrumClients.delete(clientId);
    info("thrum.disconnected", { clientId: clientId.slice(0, 8), total: thrumClients.size });
  });

  socket.on("error", (err) => {
    trace("thrum.socket.failed", { err: String(err) });
  });
});

thrumServer.listen(HUM, () => {
  info("thrum.listening", { path: HUM });
});

function jsonResponse(res: ServerResponse, data: unknown, status = 200): void {
  const body = JSON.stringify(data);
  res.writeHead(status, { "Content-Type": "application/json", "Content-Length": Buffer.byteLength(body) });
  res.end(body);
}

function readBody(req: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    let data = "";
    req.on("data", (chunk: Buffer) => { data += chunk.toString(); });
    req.on("end", () => resolve(data));
    req.on("error", reject);
  });
}

const httpServer = createHttpServer(async (req, res) => {
  const url = new URL(req.url ?? "/", "http://localhost");

  if (req.method === "GET" && url.pathname === "/status") {
    const droneStates: Record<string, unknown> = {};
    for (const [s, state] of drone.inspect()) {
      if (state.localWane === 0 && state.remoteWane === 0 && state.missedBeats === 0) continue;
      droneStates[s] = {
        assessment: state.assessment, rhythm: state.rhythm,
        localWane: state.localWane, remoteWane: state.remoteWane,
        missedBeats: state.missedBeats, pendingEchoes: state.pendingEchoes.size,
        inflightTools: state.inflightTools, pendingPermissions: state.pendingPermissions,
        suspicious: state.suspicious,
      };
    }
    return jsonResponse(res, { pid: process.pid, procs: nest.survey(), sessions: sessions.size, drone: droneStates });
  }

  if (req.method === "GET" && url.pathname === "/savings") {
    const sessionCtx: Array<{ sid: string; maxContextTokens: number }> = [];
    for (const [sid, sess] of sessions) {
      if (sess.maxContextTokens && sess.maxContextTokens > 0) sessionCtx.push({ sid, maxContextTokens: sess.maxContextTokens });
    }
    sessionCtx.sort((a, b) => b.maxContextTokens - a.maxContextTokens);
    pennySave(PENNY_FILE);
    // Estimate dollars saved — conservative, based on sonnet pricing
    const INPUT_PRICE = 3 / 1_000_000;   // $3/MTok
    const OUTPUT_PRICE = 15 / 1_000_000;  // $15/MTok
    const COMPACTION_OUTPUT_TOKENS = 2000;
    // Average context at compaction time — use top session as proxy
    const avgCtx = sessionCtx.length > 0 ? sessionCtx[0].maxContextTokens : 150_000;
    const compactionSaved = penny.curateEvents * (avgCtx * INPUT_PRICE + COMPACTION_OUTPUT_TOKENS * OUTPUT_PRICE);
    // JSONL bytes pruned → tokens not cache-read on subsequent turns
    const pruneSaved = (penny.curateBytesSaved / 4) * 0.3 / 1_000_000 * penny.blooms; // cache-read price × turns after prune
    const estimatedSaved = compactionSaved + pruneSaved;
    return jsonResponse(res, {
      uptimeMs: Date.now() - penny.started,
      counters: penny,
      estimated: {
        dollarsSaved: Math.round(estimatedSaved * 100) / 100,
        compactionsSaved: penny.curateEvents,
        corruptionsCaught: penny.droneWithers,
        editsBlocked: penny.validationRejected + penny.bashWriteBlocked,
        jsonlBytesPruned: penny.curateBytesSaved,
      },
      contextWarnThreshold: CONTEXT_WARN_THRESHOLD,
      topContextSessions: sessionCtx.slice(0, 10),
    });
  }

  if (req.method === "POST" && url.pathname === "/savings/reset") {
    pennyReset(); pennySave(PENNY_FILE);
    return jsonResponse(res, { ok: true, resetAt: penny.started });
  }

  if (req.method === "GET" && url.pathname === "/drift") {
    const sid = url.searchParams.get("sid") ?? undefined;
    const limit = Math.max(1, Math.min(2000, parseInt(url.searchParams.get("limit") ?? "20", 10) || 20));
    const days = parseInt(url.searchParams.get("days") ?? "0", 10);
    const since = parseInt(url.searchParams.get("since") ?? "0", 10);
    let recent = drift.recent(sid, limit);
    if (days > 0 || since > 0) {
      const fromMs = since > 0 ? since : Date.now() - days * 86_400_000;
      const fromDisk = drift.readSince(fromMs, sid);
      // Merge: ring may overlap with disk for the current day. Dedupe by bloomId.
      const seen = new Set<string>();
      const merged: typeof recent = [];
      for (const t of [...fromDisk, ...recent]) {
        if (seen.has(t.bloomId)) continue;
        seen.add(t.bloomId);
        merged.push(t);
      }
      recent = merged.slice(-limit).reverse();
    }
    const aggregate = drift.aggregate(100);
    const days_avail = drift.listDays();
    return jsonResponse(res, { recent, aggregate, days: days_avail });
  }

  if (req.method === "GET" && url.pathname === "/sessions") {
    const out: Record<string, unknown> = {};
    for (const [sid, s] of sessions) out[sid] = s;
    return jsonResponse(res, out);
  }

  if (req.method === "POST" && url.pathname === "/") {
    try {
      const body = JSON.parse(await readBody(req)) as { action: string; opencodeSessionId: string };
      if (body.action === "cleanup") {
        const session = sessions.get(body.opencodeSessionId);
        if (session) {
          nest.fell(body.opencodeSessionId, body.opencodeSessionId);
          releaseDroneSession(sigil(body.opencodeSessionId));
          sessions.delete(body.opencodeSessionId);
          saveSessions(body.opencodeSessionId);
          trace("session.cleaned", { sid: body.opencodeSessionId });
        }
      }
      res.writeHead(200); res.end("ok");
    } catch { res.writeHead(400); res.end("error"); }
    return;
  }

  // Default fallback. Root path returns identity; everything else is 404
  // so callers (e.g. `hum drift`) can detect a missing route instead of
  // silently parsing the identity response as JSON.
  if (url.pathname === "/" || url.pathname === "") {
    res.writeHead(200); res.end("hum");
  } else {
    res.writeHead(404, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ error: `unknown route: ${url.pathname}`, hint: "daemon may be outdated; try `hum restart`" }));
  }
});
httpServer.listen(HTTP, () => { info("http.listening", { path: HTTP }); });

// ─── MCP HTTP Server (persistent, no cold start) ────────────────────────────

import { handleMcpRequest, setCwd as mcpSetCwd, setPermissions as mcpSetPerms, setAllowedTools as mcpSetAllowed, setPermissionCallback, setMetaCallback, setExternalTools, clearExternalTools, setMcpServerConfigs, clearMcpServerConfigs, setVisibleTools, clearVisibleTools, clearReadCache, setTendrilCallback, resolveTendril, type ExternalToolDef } from "../mcp/tools.ts";

// Fixed port so the plugin (and anything else local) can reach the MCP
// HTTP endpoint without discovery. Override with HUM_MCP_PORT if the
// default clashes with something on your machine. 29147 is in the IANA
// user range, not commonly assigned. No `|| 29147` fallback — if you
// explicitly set the env var, we respect it or fail loudly, not silently
// revert to the default.
const MCP_PORT = parseInt(process.env.HUM_MCP_PORT ?? "29147");

const MCP_HOST = process.env.HUM_HOST ?? "127.0.0.1";

const mcpServer = createHttpServer(async (req, res) => {
  const url = new URL(req.url ?? "/", `http://${MCP_HOST}:${MCP_PORT}`);

  if (req.method === "POST" && url.pathname === "/permission-check") {
    try {
      const body = JSON.parse(await readBody(req)) as { tool_name?: string; tool_input?: Record<string, unknown>; session_id?: string };
      const toolName = ((body.tool_name ?? "") as string).replace("mcp__hum__", "");
      const path = (body.tool_input?.file_path ?? body.tool_input?.path) as string | undefined;
      const sessionId = body.session_id as string;

        // Find OC session for this Claude session
        let ocSessionId: string | undefined;
        for (const [id, s] of sessions) {
          if (s.claudeSessionId === sessionId || id === sessionId) {
            ocSessionId = id;
            break;
          }
        }

        const action = getPermissionAction(toolName, path);
        trace("permission.hook.checked", { tool: toolName, path, action, ocSid: ocSessionId });

        const hookAllow = () => jsonResponse(res, {
          hookSpecificOutput: { hookEventName: "PreToolUse", permissionDecision: "allow" },
        });
        const hookDeny = (reason: string) => jsonResponse(res, {
          hookSpecificOutput: { hookEventName: "PreToolUse", permissionDecision: "deny", permissionDecisionReason: reason },
        });

        if (action === "allow") return hookAllow();
        if (action === "deny") return hookDeny("Denied by session permission rules");
        if (action === "ask") return hookAllow();

        const askId = randomUUID();
        trace("permission.hold.created", { id: askId, tool: toolName, path });
        thrum(ocSessionId ?? sessionId, { chi: "permission-ask", askId, tool: toolName, path, input: body.tool_input ?? {}, dusk: Date.now() + cfg.permissionDusk });

        const decision = await new Promise<"allow" | "deny">((resolve) => {
          HUM_PERMIT_HOLD.set(askId, { resolve, tool: toolName, path, sessionId: ocSessionId ?? sessionId, createdAt: Date.now() });
          setTimeout(() => {
            if (HUM_PERMIT_HOLD.has(askId)) {
              recordPermitHoldSpan(askId);
              HUM_PERMIT_HOLD.delete(askId);
              trace("permission.hold.timeout", { id: askId });
              resolve("deny");
            }
          }, cfg.permissionDusk);
        });

        // Caller is in `await` over resolve() — the resolve path runs from
        // /permission-respond which uses the release-permit thrum case above
        // and records the span there. For the timeout path we recorded
        // above; for the success path we record now since this lambda owns
        // the wait.
        if (HUM_PERMIT_HOLD.has(askId)) {
          recordPermitHoldSpan(askId);
          HUM_PERMIT_HOLD.delete(askId);
        }
        trace("permission.hold.resolved", { id: askId, decision });
        return decision === "allow" ? hookAllow() : hookDeny("Denied by user");
      } catch (e) {
        trace("permission.hook.failed", { err: String(e) });
        return jsonResponse(res, {
          hookSpecificOutput: { hookEventName: "PreToolUse", permissionDecision: "deny", permissionDecisionReason: "Permission check failed" },
        });
      }
    }

    if (req.method === "GET" && url.pathname === "/permission-pending") {
      const pending = Array.from(HUM_PERMIT_HOLD.entries()).map(([id, p]) => ({
        id, tool: p.tool, path: p.path, sessionId: p.sessionId,
      }));
      return jsonResponse(res, pending);
    }

    if (req.method === "POST" && url.pathname === "/permission-respond") {
      try {
        const body = JSON.parse(await readBody(req)) as { id?: string; decision: "allow" | "deny" };
        const id = body.id ?? HUM_PERMIT_HOLD.keys().next().value;
        if (!id || !HUM_PERMIT_HOLD.has(id as string)) {
          return jsonResponse(res, { error: "no active permit hold" }, 404);
        }
        const hold = HUM_PERMIT_HOLD.get(id as string)!;
        HUM_PERMIT_HOLD.delete(id as string);
        hold.resolve(body.decision);
        trace("permission.responded", { id, decision: body.decision });
        return jsonResponse(res, { ok: true });
      } catch {
        return jsonResponse(res, { error: "bad request" }, 400);
      }
    }

    if (req.method !== "POST") { res.writeHead(200); res.end("hum-mcp"); return; }
    const mcpSessionId = url.pathname.match(/^\/s\/([^/]+)/)?.[1] ?? undefined;
    try {
      const body = JSON.parse(await readBody(req)) as { jsonrpc: string; id?: number | string; method: string; params?: unknown };
      trace("mcp.request.received", { method: body.method, sid: mcpSessionId });
      const tendrilStart = body.method === "tools/call" ? Date.now() : 0;
      const tendrilName = body.method === "tools/call"
        ? ((body.params as { name?: string } | undefined)?.name ?? "unknown")
        : undefined;
      const result = await handleMcpRequest(body, mcpSessionId);
      if (tendrilStart && mcpSessionId && tendrilName && tendrilName !== "permission_prompt") {
        drift.tendril(mcpSessionId, tendrilName, Date.now() - tendrilStart);
      }
      if (!result) { res.writeHead(204); res.end(); return; }
      return jsonResponse(res, result);
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      trace("mcp.request.failed", { err: msg });
      return jsonResponse(res, { jsonrpc: "2.0", error: { code: -32700, message: msg } });
    }
  });
  mcpServer.listen(MCP_PORT, MCP_HOST, () => { info("mcp.listening", { port: MCP_PORT, host: MCP_HOST }); });

const MCP_URL = `http://${MCP_HOST}:${MCP_PORT}`;
mcpSetCwd(process.env.HUM_CWD ?? process.env.HOME ?? "/");

// Wire permission prompt MCP tool to daemon's permission logic
setPermissionCallback(async (toolName: string, input: Record<string, unknown>, sessionId?: string) => {
  const tool = toolName.replace("mcp__hum__", "");
  const path = (input?.file_path ?? input?.path ?? input?.pattern) as string | undefined;
  const action = getPermissionAction(tool, path);
  trace("permission.mcp.checked", { tool, path, action, sessionId });

  if (action === "allow") return { decision: "allow" as const };
  if (action === "deny") return { decision: "deny" as const };

  // "ask" — hold MCP response, send permission_ask via the thrum
  // so the provider can emit a hum_permission tool call to trigger OC's ctx.ask() dialog
  const askId = randomUUID();
  trace("permission.ask.hold", { id: askId, tool, path, sessionId });

  // Route to the session's thrum client — sessionId comes from MCP URL path
  thrum(sessionId ?? "", { chi: "permission-ask", askId, tool, path, input, dusk: Date.now() + cfg.permissionDusk });

  return new Promise<{ decision: "allow" | "deny" }>((resolve) => {
    HUM_PERMIT_HOLD.set(askId, {
      resolve: (decision) => resolve({ decision }),
      tool, path, sessionId: sessionId ?? "",
      createdAt: Date.now(),
    });

    // Auto-allow after 5s. The MCP permission_prompt blocks Claude CLI's stream,
    // creating a deadlock: provider can't emit hum_permission tool-call until
    // the stream flows, but the stream can't flow until MCP returns.
    // The timeout breaks the deadlock. 5s is enough for release-permit to arrive
    // if OC's ctx.ask() resolves quickly (which it does when agent auto-allows).
    setTimeout(() => {
      if (HUM_PERMIT_HOLD.has(askId)) {
        recordPermitHoldSpan(askId);
        HUM_PERMIT_HOLD.delete(askId);
        trace("permission.hold.timeout.allow", { id: askId });
        resolve({ decision: "allow" });
      }
    }, 5_000);
  });
});

// Tendril: tool calls that reach across the thrum back to OC for execution.
// The daemon holds the MCP response, hums the call to the plugin, plugin
// executes via OC's tool pipeline, hums the result back. Same shape as
// the permission hold but for actual tool execution (task subtasks etc.).
setTendrilCallback((tool, args, callId, sessionId) => {
  // Force uncup cupped petals so the provider has tool events in buds
  // before the tendril-reach signal arrives. Without this, short prompts
  // (under CUP_THRESHOLD chars) keep tool_use chunks cupped — the provider
  // sees tendril-reach with empty buds and can't emit providerExecuted=false.
  if (sessionId) {
    const session = sessions.get(sessionId);
    if (session?.forceUncup) session.forceUncup();
  }
  trace("tendril.reach", { tool, callId, sid: sessionId });
  thrum(sessionId ?? "", { chi: "tendril-reach", tool, args, callId });
});

// Wire tool metadata to thrum — OC gets it out-of-band, Claude CLI never sees it
setMetaCallback((toolName, callId, title, metadata) => {
  // Find the active session to thrum to
  for (const [sid, session] of sessions) {
    const roost = nest.roost(sid);
    if (roost && roost.activeSid === sid) {
      thrum(sid, { chi: "tool-meta", tool: toolName, callId, title, metadata });
      trace("meta.hummed", { tool: toolName, sid });
      return;
    }
  }
  // No active session — broadcast to all thrum clients
  for (const client of thrumClients.values()) {
    thrumTo(client, { chi: "tool-meta", tool: toolName, callId, title, metadata });
  }
  trace("meta.hummed.broadcast", { tool: toolName });
});

// ─── Auto-update ─────────────────────────────────────────────────────────────

const CURRENT_VERSION = (() => {
  try {
    const pkg = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "..", "..", "package.json"), "utf-8");
    return JSON.parse(pkg).version as string;
  } catch { return "0.0.0"; }
})();

async function checkForUpdate(): Promise<void> {
  try {
    // Check if gh is available
    const which = nodeSpawnSync("which", ["gh"], { stdio: ["pipe", "pipe", "pipe"] });
    if (which.status !== 0) return;

    const result = nodeSpawnSync("gh", ["release", "view", "--repo", "adiled/hum", "--json", "tagName", "-q", ".tagName"], {
      stdio: ["pipe", "pipe", "pipe"],
    });
    if (result.status !== 0) return;

    const latest = result.stdout.toString().trim().replace(/^v/, "");
    if (!latest || latest === CURRENT_VERSION) return;

    info("update-available", { current: CURRENT_VERSION, latest });

    const humBin = join(process.env.HOME ?? "/", ".local", "bin", "hum");
    const update = nodeSpawn(humBin, ["update"], { stdio: "inherit" });
    await new Promise<void>(resolve => update.on("exit", () => resolve()));
  } catch {}
}

// Check every 6 hours
const UPDATE_INTERVAL = 6 * 60 * 60 * 1000;
setTimeout(checkForUpdate, 60_000); // first check 1 min after boot
setInterval(checkForUpdate, UPDATE_INTERVAL);

process.on("SIGINT",  () => { nest.silence(); process.exit(0); });
process.on("SIGTERM", () => { nest.silence(); process.exit(0); });
process.on("uncaughtException",  e => info("process.uncaught", { err: String(e) }));
process.on("unhandledRejection", e => info("process.unhandled", { err: String(e) }));

drift.configure({
  storeDir: `${STATE_DIR}/drift`,
  retentionDays: cfg.driftRetentionDays,
  version: CURRENT_VERSION,
});
setInterval(() => drift.prune(), 86_400_000); // daily prune

info("ready", { http: HTTP, mcp: MCP_URL, pid: process.pid, version: CURRENT_VERSION, maxProcs: MAX_PROCS, idleTimeout: IDLE_TIMEOUT, droned: DRONED });

// ─── Session Reaper ─────────────────────────────────────────────────────────
// Remove stale sessions that haven't been accessed in a while.

const REAP_INTERVAL = 60_000; // check every 60s
const REAP_MAX_AGE = 60 * 60 * 1000; // 1 hour

function reapSessions(): void {
  const now = Date.now();
  let reaped = 0;
  for (const [sid, session] of sessions) {
    if (!session.lastAccessed) continue;
    const age = now - session.lastAccessed;
    if (age < REAP_MAX_AGE) continue;
    // Don't reap if a process is alive for this session
    if (nest.roost(sid)) continue;
    sessions.delete(sid);
    reaped++;
  }
  if (reaped > 0) {
    saveSessions();
    trace("session.reaped", { count: reaped, remaining: sessions.size });
  }
}

setInterval(reapSessions, REAP_INTERVAL);
// Reap on startup too — clean up sessions from before daemon restart
reapSessions();

// ─── Orphan Hook-Dir Sweeper ───────────────────────────────────────────────
// harness.ts creates /tmp/hum-hook-<pid>-<rand>/ per PTY spawn and cleans
// up on harness destroy. SIGKILL'd daemons leave orphans behind; over time
// these accumulate (cf. bun .so cleanup memory). Sweep once at boot — any
// dir older than an hour can't belong to an in-flight spawn from this boot.
function sweepHookDirs(): void {
  const ONE_HOUR = 60 * 60 * 1000;
  const now = Date.now();
  let removed = 0;
  try {
    const entries = readdirSync("/tmp");
    for (const name of entries) {
      if (!name.startsWith("hum-hook-")) continue;
      const path = `/tmp/${name}`;
      try {
        const st = statSync(path);
        if (!st.isDirectory()) continue;
        if (now - st.mtimeMs < ONE_HOUR) continue;
        rmSync(path, { recursive: true, force: true });
        removed++;
      } catch {
        // ignore — race with concurrent cleanup, perms, etc.
      }
    }
    if (removed > 0) trace("tmp.hookdirs.swept", { removed });
  } catch (e) {
    trace("tmp.hookdirs.swept.failed", { err: String(e) });
  }
}
sweepHookDirs();


nest = new Nest({
  cfg,
  cliPath: process.env.CLAUDE_CLI_PATH ?? "claude",
  mcpUrl: MCP_URL,
  sessions: sessions as Map<string, { needsRespawn?: boolean; claudeSessionId?: string | null }>,
  saveSessions,
  drift: { mark: (s, e) => drift.mark(s, e), span: (s, n, ms) => drift.span(s, n, ms) },
  drone: { observed: (s, e) => drone.observed(s, e as any) },
  thrum,
  thrumPulse,
  getPermissionAction,
  permitHold: HUM_PERMIT_HOLD,
  recordPermitHoldSpan,
});

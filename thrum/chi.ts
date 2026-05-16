// ─── Thrum protocol: chi registry + tone shapes ────────────────────────────
//
// One file enumerates every chi value, every tone shape, and the envelope
// they share. Touch this when the wire changes — and bump THRUM_VERSION.
//
// THRUM_VERSION is the protocol's own semver, independent of any package
// version. Bump rules:
//   - patch: docstring tweaks, optional fields added with safe defaults
//   - minor: new chi value, new required field with backward-compat path
//   - major: removed chi, renamed chi, removed field, changed semantics
//
// The daemon stamps THRUM_VERSION into its `breath` handshake; clients can
// log a warning on mismatch. The protocol itself doesn't enforce a minimum
// version yet — see `chi/handshake.ts` (not built) when that arrives.

export const THRUM_VERSION = "0.2.0";

// Every wire-known chi value. Adding a new one bumps the minor version.
export const Chi = {
  // ── Nestler → Daemon ─────────────────────────────────────────────────
  hello:         "hello",          // announce self — protoVersion, nestling, version
  prompt:        "prompt",         // start a turn — content, system, tools
  cancel:        "cancel",         // interrupt mid-turn
  cleanup:       "cleanup",        // session deleted, drop daemon state
  curate:        "curate",         // manual compaction request
  releasePermit: "release-permit", // resolve an earlier permission-ask
  tendrilResult: "tendril-result", // task subagent answered
  toolResult:    "tool-result",    // nestler-declared tool answered
  petalCell:     "petal-cell",     // OC message-graph update (graft hint)

  // ── Daemon → Nestler ─────────────────────────────────────────────────
  breath:        "breath",         // handshake — full state sync on connect
  chunk:         "chunk",          // model output partwise (text/reasoning/tool)
  finish:        "finish",         // turn complete — finishReason + usage
  error:         "error",          // turn aborted
  sessionReady:  "session-ready",  // nest spawned, claude session id known
  pulse:         "pulse",          // process lifecycle event
  permissionAsk: "permission-ask", // mid-stream permission needed
  tendrilReach:  "tendril-reach",  // task subagent dispatch
  toolCall:      "tool-call",      // nestler-declared tool dispatch
  toolMeta:      "tool-meta",      // out-of-band metadata for a tool result

  // ── Either direction ──────────────────────────────────────────────────
  echo:          "echo",           // delivery ack for a rid
  perfMark:      "perf-mark",      // drift timing — measured both ways
  log:           "log",            // structured log forwarding
  drone:         "drone",          // drone heartbeat (daemon → nestler)
  droneRetrofit: "drone-retrofit", // drone swallow + retry signal
} as const;

export type ChiKind = typeof Chi[keyof typeof Chi];

export const ALL_CHI: ReadonlySet<ChiKind> = new Set(Object.values(Chi));
export function isValidChi(s: string): s is ChiKind { return ALL_CHI.has(s as ChiKind); }

// ─── Pulse kinds ─────────────────────────────────────────────────────────
// pulse.kind is its own enum within chi:"pulse" tones.
export const PulseKind = {
  roostSpawned:  "roost-spawned",  // process created
  roostReady:    "roost-ready",    // system init received, accepting input
  roostIdle:     "roost-idle",     // turn complete, no listeners
  roostDied:     "roost-died",     // process exited
  roostEvicted:  "roost-evicted",  // killed to make room
} as const;
export type PulseKindT = typeof PulseKind[keyof typeof PulseKind];

// ─── Envelope ────────────────────────────────────────────────────────────
// Fields every tone may carry. `chi` and `rid` are required; the rest are
// situational. Validators reject tones missing required fields at the
// receive boundary.

export interface Envelope {
  chi: ChiKind;
  rid: string;          // correlation id — required, unique per send
  from?: string;        // sender identity
  to?: string;          // recipient identity (omit = sid-routed or broadcast)
  sigil?: string;       // sentinel pairing hash for this sid
  sid?: string;         // hum session id (most tones have one)
  wane?: number;        // sender's wane for this sigil at send time
  sentAt?: number;      // ms timestamp — for drift attribution
  dusk?: number;        // absolute expiry — past this, drop tone

  // Nestling-private extension bag. Each nestling owns its own key:
  //   ext.opencode.serverUrl, ext.vercel-ai.someField, …
  // Thrum core ignores ext; the receiving nestling (or humd, when it
  // needs to bridge to a specific nestler) reads its own slot.
  // Always optional. Universal concepts live as top-level fields; only
  // truly nestling-private state belongs here.
  ext?: Record<string, Record<string, unknown>>;
}

// ─── Tone shapes ─────────────────────────────────────────────────────────
// Discriminated by `chi`. Each shape names ONLY the chi-specific fields;
// envelope fields above are merged in.

export interface HelloTone extends Envelope {
  chi: "hello";
  // Nestler's announcement on connect. All optional — daemon should
  // tolerate clients that say nothing more than `chi: "hello"`.
  protoVersion?: string;   // nestler's thrum protocol version
  nestling?: string;       // representation name: "opencode", "vercel-ai", "openai-server", "grpc", …
  nestlerVersion?: string; // package version of the nestling
}

// A petal is one unit of conversation content (text, image, tool_use,
// tool_result, …). priorPetals on a prompt is hum's vocabulary for
// "the history that arrived before this turn." Each nestling sources
// its petals however it likes — OC walks `client.session.messages`,
// another might read its own conversation store.
export type Petal = Record<string, unknown>;

// External MCP server config — hum's universal way to declare an MCP
// server the daemon should connect to on the nestler's behalf. Same
// shape whether the nestler discovered it via OC config, env vars,
// or its own UI.
export type ExternalMcp =
  | { name: string; type: "local"; command: string[]; environment?: Record<string, string> }
  | { name: string; type: "remote"; url: string; headers?: Record<string, string> };

export interface PromptTone extends Envelope {
  chi: "prompt";
  sid: string;
  modelId: string;
  // The current turn's user input. `content` is the structured form
  // (text + image parts etc.); `text` is the flat string shortcut.
  content?: string | Array<Record<string, unknown>>;
  text?: string;
  systemPrompt?: string;
  // Working directory humd binds this hum to. Drives transcript
  // location and hum-fs MCP grounding. Omit for pure inference.
  cwd?: string;
  nestling?: string;
  nest?: "claude-repl" | "claude-cli";
  hearOnly?: boolean;
  // Nestler-declared tools. The daemon advertises these to the model
  // via MCP and forwards calls back to the nestler over thrum.
  tools?: Array<{ name: string; description?: string; parameters?: unknown }>;
  // Per-hum permission rules (allow/deny patterns). Universal shape;
  // each nestler maps its own permission model onto this.
  permissions?: unknown[];
  // Subset of tool names the model is allowed to see this turn.
  allowedTools?: string[];
  // Conversation history known to the nestler at send time. Daemon
  // grafts new petals into the nest's transcript on receive.
  priorPetals?: Petal[];
  // Skip the graft step for this turn — used when the nestler knows
  // the transcript already matches its view (e.g. mid-tool returns).
  skipGraft?: boolean;
  planMode?: boolean;
  // External MCP servers the daemon should connect to for this hum.
  // Daemon executes their tools directly, surfaces results to the model.
  externalMcps?: ExternalMcp[];
  // Tool names + schemas served by `externalMcps`. Universal hum
  // optimization: a nestler that already knows them spares the daemon
  // a synchronous tools/list discovery round-trip per spawn. Omit and
  // the daemon will discover them on demand (TODO).
  externalTools?: Array<{ name: string; description?: string; inputSchema: Record<string, unknown> }>;
  // Tool names that should be visible to the model this turn — a
  // filter over hum's native tools + nestler-declared tools + external
  // MCP tools. Omit to advertise all.
  visibleToolNames?: string[];
  // Penny is hum's lifetime accounting. Nestlers ship counter
  // increments (number of token swaps, tool exec counts, etc.) that
  // the daemon merges into its persistent ledger.
  pennyDelta?: Record<string, number>;
}

export interface CancelTone extends Envelope {
  chi: "cancel";
  sid: string;
  reason?: string;
}

export interface CleanupTone extends Envelope {
  chi: "cleanup";
  sid: string;
}

export interface CurateTone extends Envelope {
  chi: "curate";
  sid: string;
}

export interface ReleasePermitTone extends Envelope {
  chi: "release-permit";
  askId: string;
  decision: "allow" | "deny";
}

export interface TendrilResultTone extends Envelope {
  chi: "tendril-result";
  callId: string;
  result: string;
}

export interface ToolResultTone extends Envelope {
  chi: "tool-result";
  sid: string;
  callId: string;
  result: string;
}

// A petal-cell is the nestler's view of one unit of its conversation
// state — the nestler's analog to a JSONL entry in the nest's
// transcript. Cell is hum's vocab for "the addressable container
// around a petal." Nestlers ship petal-cell tones whenever their
// view of the hum's conversation graph changes, so the daemon can
// keep the nest's transcript aligned (graft, prune, retag).
export interface PetalCellTone extends Envelope {
  chi: "petal-cell";
  sid: string;
  event: string;            // what changed: "updated", "removed", …
  role?: string;            // petal role: user / assistant / tool / system
  model?: string;            // model id at the time of the petal
  provider?: string;         // provider id at the time of the petal
  messageId?: string;        // nestler's id for this petal
  parentId?: string;         // parent in the nestler's graph
  completed?: number;        // ms timestamp the petal finalized
}

export interface BreathSessionView {
  sigil: string;
  sid: string;
  nestId: string | null;
  nestPath: string | null;
  lastSyncedPetal: string | null;
  wane: number;
  modelId: string;
  cwd?: string;
  roostAlive: boolean;
  roostPid?: number;
}

export interface BreathTone extends Envelope {
  chi: "breath";
  sessions: BreathSessionView[];
  protoVersion?: string; // daemon advertises THRUM_VERSION here
}

export interface ChunkTone extends Envelope {
  chi: "chunk";
  sid: string;
  chunkType:
    | "text_start" | "text_delta"
    | "reasoning_start" | "reasoning_delta" | "reasoning_end"
    | "tool_input_start" | "tool_input_delta" | "tool_call" | "tool_result"
    | "content_block_stop" | "stream_start";
  delta?: string;
  toolCallId?: string;
  toolName?: string;
  partialJson?: string;
  input?: unknown;
  result?: unknown;
}

export interface FinishTone extends Envelope {
  chi: "finish";
  sid: string;
  finishReason: string;
  usage?: Record<string, number>;
  providerMetadata?: Record<string, unknown>;
}

export interface ErrorTone extends Envelope {
  chi: "error";
  sid: string;
  message: string;
}

export interface SessionReadyTone extends Envelope {
  chi: "session-ready";
  sid: string;
  nestId: string;
  model: string;
  tools: string[];
}

export interface PulseTone extends Envelope {
  chi: "pulse";
  kind: PulseKindT;
  sigil: string;
  sid: string;
  pid?: number;
  reason?: string;
}

export interface PermissionAskTone extends Envelope {
  chi: "permission-ask";
  askId: string;
  tool: string;
  path?: string;
  input: unknown;
}

export interface TendrilReachTone extends Envelope {
  chi: "tendril-reach";
  tool: string;
  args: Record<string, unknown>;
  callId: string;
}

export interface ToolCallTone extends Envelope {
  chi: "tool-call";
  sid: string;
  name: string;
  args: Record<string, unknown>;
  callId: string;
}

export interface ToolMetaTone extends Envelope {
  chi: "tool-meta";
  sid: string;
  tool: string;
  callId: string;
  title?: string;
  metadata?: Record<string, unknown>;
}

export interface EchoTone {
  chi: "echo";
  rid: string;
  ok: boolean;
  error?: string;
}

export interface PerfMarkTone extends Envelope {
  chi: "perf-mark";
  sid: string;
  mark?: string;
  span?: { name: string; ms: number };
  thrum?: { to: string; ms: number };
}

export interface LogTone extends Envelope {
  chi: "log";
  level: "trace" | "info" | "warn" | "error";
  event: string;
  data?: Record<string, unknown>;
}

export interface DroneTone extends Envelope {
  chi: "drone";
  sigil: string;
  wane: number;
  assessment: { unified: string; raw: string };
  rhythm: number;
  pendingEchoes: string[];
  load: { activeSessions: number; pendingPermissions: number; inflightTools: number; tokensBurned: number };
}

export interface DroneRetrofitTone extends Envelope {
  chi: "drone-retrofit";
  sid: string;
  reason: string;
}

// Discriminated union of every wire-typed tone. Use `Tone` for inbound
// parsing where the chi switches behavior; the loose `LooseTone` is the
// fallback for code that still treats tones as bag-of-fields.
export type Tone =
  | HelloTone
  | PromptTone | CancelTone | CleanupTone | CurateTone
  | ReleasePermitTone | TendrilResultTone | ToolResultTone | PetalCellTone
  | BreathTone | ChunkTone | FinishTone | ErrorTone
  | SessionReadyTone | PulseTone | PermissionAskTone
  | TendrilReachTone | ToolCallTone | ToolMetaTone
  | EchoTone | PerfMarkTone | LogTone | DroneTone | DroneRetrofitTone;

export type LooseTone = Envelope & Record<string, unknown>;

// ─── Validators ──────────────────────────────────────────────────────────
// Cheap, structural. Reject malformed tones at the boundary instead of
// crashing later on a missing field.

export function isEnvelope(x: unknown): x is Envelope {
  if (!x || typeof x !== "object") return false;
  const o = x as Record<string, unknown>;
  return typeof o.chi === "string"
      && (typeof o.rid === "string" || o.rid === undefined) // legacy tolerance
      && (o.sid === undefined || typeof o.sid === "string");
}

export function isKnownTone(x: unknown): x is Tone {
  if (!isEnvelope(x)) return false;
  return isValidChi(x.chi as string);
}

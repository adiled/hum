import { readFileSync } from "fs";
import { join } from "path";

export interface HumConfig {
  maxProcs: number;
  /**
   * Milliseconds an idle roost stays alive before eviction.
   */
  idleTimeout: number;
  smallModel: string;
  permissionDusk: number;
  droned: boolean;
  droneModel: { providerID: string; modelID: string };
  /**
   * Default nest implementation for sessions without a per-project
   * override.
   *   "claude-repl" (default) — interactive Ink REPL through a PTY.
   *                              Usage bills against Pro/Max subscription.
   *   "claude-cli"             — legacy `-p` (print/pipe) headless mode.
   *                              Bills against API credits. Use this if
   *                              you have things covered another way, or
   *                              you want more suffering.
   * Folder layout: nests/claude-repl/ and nests/claude-cli/.
   */
  nest: "claude-repl" | "claude-cli";
  /**
   * Project registry. Stable hum-native id per primaryPath, kept for
   * future plugin-sent project resolution. Nest is dictated by the
   * nestler at handshake time — not configured per project.
   */
  projects: Array<{ id: string; primaryPath: string }>;
  /**
   * Experimental feature toggles. Off by default. Promoted into the
   * main config surface (or removed) once stable. Grouped here so new
   * experiments don't clutter the top level.
   */
  experimental: {
    /**
     * Linguistic sub-symbol addressing for do_code / read:
     * foo.when.body, foo.try.otherwise, foo.loop.body, foo.return, …
     * See lib/ast.ts — resolveAliasPath.
     */
    subpath: boolean;
  };
  /**
   * Claude CLI environment overrides. Untyped — whatever keys the user
   * supplies are spread into the nest spawn env AFTER hum's defaults,
   * so user entries override ours (e.g. re-enable CLAUDE_CODE_DISABLE_*
   * flags hum turned off, or set arbitrary CC-facing env).
   */
  ccFlags: Record<string, string>;
  /**
   * Manual compaction behavior for hum-routed sessions. Auto-compaction
   * is permanently off because hum models declare limit.context: 0 in
   * opencode.json — OC's overflow check (overflow.ts:21) skips, no
   * 'compacting…' TUI hiccup, Claude CLI's native microcompaction handles
   * real overflow.
   *   'off'    — when the user manually fires compaction in TUI, our
   *              provider stub-returns: no JSONL prune, no model call.
   *              Default.
   *   'curate' — manual compaction triggers a surgical JSONL prune
   *              (strips thinking blocks, trims old tool_result content).
   * Other providers' compaction is untouched.
   */
  compaction: 'off' | 'curate';
  /**
   * Drift retention in days. Daemon writes one NDJSON file per day to
   * ${state}/drift/YYYY-MM-DD.ndjson and prunes files older than this on
   * startup + once daily. Default 30 days. 0 disables persistence (ring
   * buffer only).
   */
  driftRetentionDays: number;
}

const DEFAULTS: HumConfig = {
  maxProcs: 4,
  idleTimeout: 30_000,
  smallModel: "",
  permissionDusk: 60_000,
  droned: false,
  droneModel: { providerID: "opencode-hum", modelID: "claude-haiku-4-5" },
  nest: "claude-repl",
  projects: [],
  experimental: {
    subpath: false,
  },
  ccFlags: {},
  compaction: 'off',
  driftRetentionDays: 30,
};

function parseProjects(raw: unknown): Array<{ id: string; primaryPath: string }> {
  if (!Array.isArray(raw)) return [];
  const out: Array<{ id: string; primaryPath: string }> = [];
  for (const entry of raw) {
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) continue;
    const e = entry as { id?: unknown; primaryPath?: unknown };
    if (typeof e.primaryPath !== "string" || e.primaryPath.length === 0) continue;
    if (typeof e.id !== "string" || e.id.length === 0) continue;
    out.push({ id: e.id, primaryPath: e.primaryPath });
  }
  return out;
}

const CONFIG_PATHS = [
  join(process.env.XDG_CONFIG_HOME ?? join(process.env.HOME ?? "/", ".config"), "hum", "hum.json"),
];

let cached: HumConfig | null = null;

export function loadConfig(): HumConfig {
  if (cached) return cached;

  for (const path of CONFIG_PATHS) {
    try {
      const raw = JSON.parse(readFileSync(path, "utf8"));
      cached = {
        maxProcs: raw.maxProcs ?? DEFAULTS.maxProcs,
        idleTimeout: raw.idleTimeout ?? DEFAULTS.idleTimeout,
        smallModel: raw.smallModel ?? DEFAULTS.smallModel,
        permissionDusk: raw.permissionDusk ?? DEFAULTS.permissionDusk,
        droned: raw.droned ?? DEFAULTS.droned,
        droneModel: raw.droneModel ?? DEFAULTS.droneModel,
        nest: raw.nest === "claude-cli" ? "claude-cli" : "claude-repl",
        projects: parseProjects(raw.projects),
        experimental: {
          subpath: raw.experimental?.subpath ?? DEFAULTS.experimental.subpath,
        },
        ccFlags: (raw.ccFlags && typeof raw.ccFlags === "object" && !Array.isArray(raw.ccFlags))
          ? Object.fromEntries(Object.entries(raw.ccFlags).map(([k, v]) => [k, String(v)]))
          : DEFAULTS.ccFlags,
        compaction: raw.compaction === 'curate' ? 'curate' : DEFAULTS.compaction,
        driftRetentionDays: typeof raw.driftRetentionDays === "number" && raw.driftRetentionDays >= 0
          ? Math.floor(raw.driftRetentionDays)
          : DEFAULTS.driftRetentionDays,
      };
      return cached;
    } catch {}
  }

  cached = { ...DEFAULTS };
  return cached;
}

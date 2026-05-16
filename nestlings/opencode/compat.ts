/**
 * OpenCode version compat shims.
 *
 * OC's SDK has occasional shape breaks across minor versions. This file
 * isolates the version-sniff + per-version branching so provider.ts only
 * deals in stable shapes.
 *
 * Notable break: OC 1.15.0 renamed `path: { sessionID }` → `path: { id }`
 * on the session.* endpoints. Old SDK clients silently sent the literal
 * URL "/session/{id}" because no substitution kicked in, which the new
 * server rejects as "Expected a string starting with 'ses', got '%7Bid%7D'".
 *
 * Self-contained: no imports from provider.ts. The caller injects a
 * trace logger via `setCompatTrace` so detection events show up in the
 * usual thrum/file log stream without creating a circular module graph.
 */

import { spawnSync } from "child_process";

// Module-local cache. Detection is idempotent — called once per plugin load,
// result memoized for the process lifetime.
let ocVersion: string | null = null;
let ocVersionParsed: [number, number, number] | null = null;
// Latch so we only log the unknown-version warning once across the process.
let unknownVersionWarned = false;

// Optional trace sink. Provider sets this during init so compat events
// land in the same log stream as everything else. Defaults to a no-op so
// compat can be imported in isolation (tests, dry runs).
type TraceFn = (event: string, data?: Record<string, unknown>) => void;
let traceFn: TraceFn = () => {};
export function setCompatTrace(fn: TraceFn): void { traceFn = fn; }

function parseSemver(v: string): [number, number, number] | null {
  const m = v.trim().match(/^v?(\d+)\.(\d+)\.(\d+)/);
  if (!m) return null;
  return [parseInt(m[1], 10), parseInt(m[2], 10), parseInt(m[3], 10)];
}

function gteSemver(a: [number, number, number], b: [number, number, number]): boolean {
  for (let i = 0; i < 3; i++) {
    if (a[i] > b[i]) return true;
    if (a[i] < b[i]) return false;
  }
  return true;
}

/**
 * Detect the OpenCode binary version.
 *
 * Preferred source: `pluginInput.app?.version` — zero-cost, supplied by the
 * plugin host directly. Current @opencode-ai/plugin (as of writing) does NOT
 * expose this field, but the call is wrapped defensively so future hosts that
 * do can drop in without a code change.
 *
 * Fallback: shell out `opencode --version`. ~30ms cold, then memoized.
 */
export function detectOcVersion(pluginInput?: unknown): string | null {
  if (ocVersion) return ocVersion;

  // Fast path: plugin host supplies version inline.
  const fromInput = (pluginInput as { app?: { version?: unknown } } | undefined)?.app?.version;
  if (typeof fromInput === "string" && fromInput.trim()) {
    const parsed = parseSemver(fromInput);
    if (parsed) {
      ocVersion = fromInput.match(/^v?(\d+\.\d+\.\d+)/)?.[1] ?? fromInput;
      ocVersionParsed = parsed;
      traceFn("oc.version.detected", { version: ocVersion, source: "pluginInput" });
      return ocVersion;
    }
  }

  // Slow path: ask the binary.
  try {
    const r = spawnSync("opencode", ["--version"], { encoding: "utf-8", timeout: 3000 });
    const out = (r.stdout ?? "").trim();
    const parsed = parseSemver(out);
    if (parsed) {
      ocVersion = out.match(/^v?(\d+\.\d+\.\d+)/)?.[1] ?? out;
      ocVersionParsed = parsed;
      traceFn("oc.version.detected", { version: ocVersion, source: "spawn" });
    }
  } catch (e) {
    traceFn("oc.version.failed", { err: String(e) });
  }
  return ocVersion;
}

/**
 * Session path-param shape, branched on detected OC version.
 *
 * - Detected >= 1.15.0  → `{ id }` (new shape)
 * - Detected <  1.15.0  → `{ sessionID }` (old shape)
 * - Detection failed    → `{ sessionID }` (old shape) + one-shot warning
 *
 * Defaulting to the old shape on detection failure is deliberate: a wrong
 * guess on a pre-1.15 user fails silently with a confusing "/session/{id}"
 * URL substitution miss, whereas a wrong guess on a 1.15+ user fails loudly
 * with a clear "unknown field sessionID" error. Loud > silent.
 */
export function sessionPathParam(sessionId: string): Record<string, string> {
  if (ocVersionParsed) {
    return gteSemver(ocVersionParsed, [1, 15, 0])
      ? { id: sessionId }
      : { sessionID: sessionId };
  }
  if (!unknownVersionWarned) {
    unknownVersionWarned = true;
    traceFn("oc.version.unknown", { fallback: "sessionID" });
  }
  return { sessionID: sessionId };
}

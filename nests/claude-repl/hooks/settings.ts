// Build the --settings JSON Claude CLI expects for our SessionStart/Stop hooks,
// and merge it into any pre-existing --settings flag the caller already passed.

export function buildHookSettings(scriptPath: string): string {
  const event = (name: string) => ({
    matcher: "*",
    hooks: [{ type: "command", command: `${scriptPath} ${name}` }],
  });
  // Only Stop is registered. SessionStart used to be our "Ink is up,
  // type the prompt now" signal, but it fires before Ink commits to
  // the input-box state in some startup paths (welcome banner with the
  // "you launched claude in your home directory" note, opus-4-5 cold
  // start in large workspaces) — bytes injected then land on a stale
  // frame and Enter is swallowed. Readiness is now driven by polling
  // the actual screen for the input-bar signature; see harness.ts.
  //
  // skipDangerousModePermissionPrompt suppresses the bypass-permissions
  // disclaimer modal that Claude CLI renders on every spawn when
  // --dangerously-skip-permissions is passed. Setting it inline (rather
  // than relying on ~/.claude/settings.json) keeps the behavior
  // portable across hosts and survives hum reinstalls.
  return JSON.stringify({
    skipDangerousModePermissionPrompt: true,
    hooks: {
      Stop: [event("Stop")],
    },
  });
}

// If caller already passed --settings, append the hook settings JSON
// by merging; otherwise add --settings <hookJson>. Claude CLI accepts
// --settings as inline JSON; later occurrences override earlier ones,
// so we MERGE shallowly with hooks taking the last-write-wins slot.
export function injectSettingsArg(args: string[], hookJson: string): string[] {
  const existingIdx = args.indexOf("--settings");
  if (existingIdx >= 0 && existingIdx + 1 < args.length) {
    try {
      const existing = JSON.parse(args[existingIdx + 1]);
      const hook = JSON.parse(hookJson);
      const merged = { ...existing, ...hook, hooks: { ...(existing.hooks ?? {}), ...hook.hooks } };
      const next = [...args];
      next[existingIdx + 1] = JSON.stringify(merged);
      return next;
    } catch {
      // fall through — append a second --settings so Claude takes hook one
    }
  }
  return [...args, "--settings", hookJson];
}

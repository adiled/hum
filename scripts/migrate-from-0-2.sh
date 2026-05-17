#!/usr/bin/env bash
# migrate-from-0-2.sh — port a v0.2 hum install to v0.3.
#
# v0.2 shipped:
#   - TypeScript humd at $HUM_DATA/src/humd/humd.ts run via tsx
#   - systemd unit ExecStart pointing at node + tsx + humd.ts
#   - $HUM_STATE/sessions.json (legacy nestled shape)
#   - Pre-existing $HUM_STATE/hums.json from the rebrand inside v0.2 already
#
# v0.3 needs:
#   - Rust humd binary at $HOME/.local/bin/humd
#   - $HUM_STATE/humd.key (Ed25519 identity — minted fresh on first boot)
#   - $HUM_CONFIG/peers.json (ensemble peer list)
#   - systemd unit pointing at the Rust binary
#
# This script:
#   1. Stops the v0.2 daemon hard.
#   2. Backs up legacy state in case rollback is needed.
#   3. Removes the TypeScript source tree at $HUM_DATA/src (the binary
#      install lives elsewhere now).
#   4. Drops the legacy systemd unit so install can write a fresh one.
#   5. Leaves $HUM_STATE/hums.json and penny.json intact — the Rust
#      daemon reads the same files. Sessions migrated in v0.2 already.
#
# Idempotent. Safe to re-run.
set -euo pipefail

XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-$HOME/.config}"
XDG_DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
XDG_STATE_HOME="${XDG_STATE_HOME:-$HOME/.local/state}"
HUM_DATA="$XDG_DATA_HOME/hum"
HUM_STATE="$XDG_STATE_HOME/hum"
HUM_SRC="$HUM_DATA/src"

log()  { printf '\033[1m[migrate-0.2]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[migrate-0.2]\033[0m %s\n' "$*" >&2; }

# 1. Stop the v0.2 daemon.
if systemctl --user list-unit-files 2>/dev/null | grep -q '^hum\.service'; then
  log "stopping v0.2 hum.service"
  systemctl --user kill -s KILL hum 2>/dev/null || true
  systemctl --user stop hum 2>/dev/null || true
  systemctl --user disable hum 2>/dev/null || true
fi

# 2. Backup legacy state — non-destructive.
STAMP="$(date +%Y%m%d-%H%M%S)"
BACKUP="$HUM_STATE/backup-pre-0.3-$STAMP"
mkdir -p "$BACKUP"
for f in sessions.json hums.json penny.json; do
  if [ -s "$HUM_STATE/$f" ]; then
    cp -a "$HUM_STATE/$f" "$BACKUP/"
  fi
done
log "state backed up to $BACKUP"

# 3. Sessions → Hums shape was migrated inside v0.2 already. If a stray
#    legacy `sessions.json` lingers, hand it off to a side directory —
#    the Rust daemon doesn't read it.
if [ -s "$HUM_STATE/sessions.json" ] && [ ! -s "$HUM_STATE/hums.json" ]; then
  warn "found sessions.json with no matching hums.json — daemon will start empty."
  warn "  manual port: see backup at $BACKUP"
fi

# 4. Remove the TypeScript source tree. The Rust binary lives in
#    $HOME/.local/bin and doesn't need the repo cached.
if [ -d "$HUM_SRC" ]; then
  log "removing legacy TypeScript source at $HUM_SRC"
  rm -rf "$HUM_SRC"
fi

# 5. Drop the old systemd unit. install writes a fresh one pointing at
#    the Rust binary.
UNIT="$XDG_CONFIG_HOME/systemd/user/hum.service"
if [ -f "$UNIT" ] && grep -q 'humd\.ts' "$UNIT"; then
  log "removing legacy systemd unit at $UNIT"
  rm -f "$UNIT"
  systemctl --user daemon-reload 2>/dev/null || true
fi

log "migrate-from-0-2.sh done. install will now build v0.3."

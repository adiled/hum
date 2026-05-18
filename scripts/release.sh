#!/usr/bin/env bash
# scripts/version.sh — bump hum's version everywhere, commit, tag, push.
#
# Single source of truth: the VERSION file at repo root. Every other
# version-bearing file (Cargo.toml workspace.package.version, every
# nestling's package.json, recipes/opencode/tests/package.json) is
# rewritten to match. The thrum protocol version lives in
# `thrum-core/src/chi.rs` (THRUM_VERSION) and is independent — this
# script never touches it.
#
# Usage:
#   scripts/version.sh patch          # 0.3.0 → 0.3.1
#   scripts/version.sh minor          # 0.3.1 → 0.4.0
#   scripts/version.sh major          # 0.4.0 → 1.0.0
#   scripts/version.sh 0.3.0          # explicit; must be > current
#   scripts/version.sh 0.3.0 "msg"    # optional annotated-tag message
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION_FILE="$ROOT/VERSION"
BUMP="${1:-}"
MSG="${2:-}"

[ -n "$BUMP" ] || {
  echo "Usage: $0 {patch|minor|major|<X.Y.Z>} [\"tag message\"]" >&2
  exit 2
}
[ -f "$VERSION_FILE" ] || { echo "missing VERSION file at $VERSION_FILE" >&2; exit 1; }

CURRENT="$(tr -d '[:space:]' < "$VERSION_FILE")"

# Strict semver — only X.Y.Z. No pre-release/build metadata for now.
SEMVER='^([0-9]+)\.([0-9]+)\.([0-9]+)$'
[[ "$CURRENT" =~ $SEMVER ]] || { echo "VERSION file holds invalid semver: '$CURRENT'" >&2; exit 1; }
CUR_MAJ=${BASH_REMATCH[1]}; CUR_MIN=${BASH_REMATCH[2]}; CUR_PATCH=${BASH_REMATCH[3]}

# Resolve $BUMP to NEXT.
case "$BUMP" in
  patch) NEXT="$CUR_MAJ.$CUR_MIN.$((CUR_PATCH + 1))" ;;
  minor) NEXT="$CUR_MAJ.$((CUR_MIN + 1)).0" ;;
  major) NEXT="$((CUR_MAJ + 1)).0.0" ;;
  *)
    if [[ "$BUMP" =~ $SEMVER ]]; then
      NEXT="$BUMP"
    else
      echo "bad argument: '$BUMP' — expected patch|minor|major|X.Y.Z" >&2
      exit 2
    fi
    ;;
esac

# Validate NEXT > CURRENT (strictly greater).
gt() {
  local a="$1" b="$2"
  [[ "$a" =~ $SEMVER ]] || return 1; local a1=${BASH_REMATCH[1]} a2=${BASH_REMATCH[2]} a3=${BASH_REMATCH[3]}
  [[ "$b" =~ $SEMVER ]] || return 1; local b1=${BASH_REMATCH[1]} b2=${BASH_REMATCH[2]} b3=${BASH_REMATCH[3]}
  if (( a1 != b1 )); then (( a1 > b1 )); return; fi
  if (( a2 != b2 )); then (( a2 > b2 )); return; fi
  (( a3 > b3 ))
}
if ! gt "$NEXT" "$CURRENT"; then
  echo "refusing: $NEXT is not greater than current $CURRENT" >&2
  exit 1
fi

echo "bumping $CURRENT → $NEXT"

# 1. VERSION file
echo "$NEXT" > "$VERSION_FILE"

# 2. Cargo.toml workspace.package.version
sed -i.bak -E "/^\[workspace\.package\]/,/^\[/ { s/^version = \".*\"/version = \"$NEXT\"/ }" "$ROOT/Cargo.toml"
rm -f "$ROOT/Cargo.toml.bak"

# 3. every nestling's package.json
shopt -s nullglob
for pkg in "$ROOT"/hives/*/package.json "$ROOT"/recipes/*/tests/package.json; do
  tmp="$(mktemp)"
  jq --arg v "$NEXT" '.version = $v' "$pkg" > "$tmp"
  mv "$tmp" "$pkg"
  echo "  updated $pkg"
done
shopt -u nullglob

# 4. Cargo.lock refresh (touches version-stamped entries)
(cd "$ROOT" && cargo update --workspace --offline >/dev/null 2>&1 || cargo update --workspace >/dev/null 2>&1 || true)

# 5. commit + tag + push
git -C "$ROOT" add -A
git -C "$ROOT" commit -m "v$NEXT" >/dev/null
if [ -n "$MSG" ]; then
  git -C "$ROOT" tag -a "v$NEXT" -m "$MSG"
else
  git -C "$ROOT" tag "v$NEXT"
fi
git -C "$ROOT" push
git -C "$ROOT" push --tags

echo "released v$NEXT"

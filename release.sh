#!/usr/bin/env bash
set -euo pipefail

# release.sh - bump version, sync lockfile, tag, and push.
# Usage: ./release.sh [patch|minor|major]
#
# Examples:
#     ./release.sh patch            # bumps 0.3.2 -> 0.3.3
#     ./release.sh minor            # bumps 0.3.2 -> 0.4.0
#     ./release.sh major            # bumps 0.3.2 -> 1.0.0
#
# Core (always): Cargo.toml + Cargo.lock.

# --- Project add-ons (optional) ----------------------------------------------
# Extra files kept in version lockstep with Cargo.toml. Leave empty for plain
# Rust projects. Example: (orchd-osx/build.zig.zon orchd-apple/build.zig.zon)
# hum's hives are excluded from the workspace, so each carries its own version
# rather than inheriting workspace.package.version. Their npm packages are
# what `hum hive install` publishes, so those track the same number.
EXTRA_VERSIONED_FILES=(
  hives/anthropic-server/package.json
  hives/bash/Cargo.toml
  hives/bp7/Cargo.toml
  hives/claude-cli/Cargo.toml
  hives/claude-repl/Cargo.toml
  hives/grpc/Cargo.toml
  hives/gsm-modem/Cargo.toml
  hives/ollama-server/Cargo.toml
  hives/openai-server/package.json
  hives/paid-oracle/Cargo.toml
  hives/vercel-ai/package.json
  recipes/opencode/tests/package.json
)
# -----------------------------------------------------------------------------

BRANCH="$(git branch --show-current)"
if [[ "$BRANCH" != "main" ]]; then
  echo "error: must be on main (currently on '$BRANCH')" >&2
  exit 1
fi

# Sync with origin
git fetch origin main
git reset --hard origin/main

# Determine version
NEW_VER=""
case "${1:-patch}" in
  patch)
    OLD_VER="$(grep '^version' Cargo.toml | sed 's/.*"\(.*\)"/\1/')"
    IFS='.' read -r MAJOR MINOR PATCH <<< "$OLD_VER"
    NEW_VER="$MAJOR.$MINOR.$((PATCH + 1))"
     ;;
  minor)
    OLD_VER="$(grep '^version' Cargo.toml | sed 's/.*"\(.*\)"/\1/')"
    IFS='.' read -r MAJOR MINOR PATCH <<< "$OLD_VER"
    NEW_VER="$MAJOR.$((MINOR + 1)).0"
     ;;
  major)
    OLD_VER="$(grep '^version' Cargo.toml | sed 's/.*"\(.*\)"/\1/')"
    IFS='.' read -r MAJOR MINOR PATCH <<< "$OLD_VER"
    NEW_VER="$((MAJOR + 1)).0.0"
     ;;
  *)
    echo "error: unknown bump type '${1}' (use patch|minor|major)" >&2
    exit 1
     ;;
esac

# Files kept in version lockstep with Cargo.toml
VERSIONED_FILES=(Cargo.toml "${EXTRA_VERSIONED_FILES[@]}")

# Every versioned file must already carry OLD_VER, or versions have drifted.
for f in "${VERSIONED_FILES[@]}"; do
  if ! grep -q "\"$OLD_VER\"" "$f"; then
    echo "error: $f does not contain version $OLD_VER (files out of sync?)" >&2
    exit 1
  fi
done

# Bump versions in all lockstep files:
#   Cargo.toml      -> `version = "x.y.z"`
#   build.zig.zon   -> `.version = "x.y.z"`   (fingerprint is name-bound, unchanged)
#   package.json    -> `"version": "x.y.z"`
for f in "${VERSIONED_FILES[@]}"; do
  sed -i '' -e "s/^version = \"$OLD_VER\"/version = \"$NEW_VER\"/" \
            -e "s/\.version = \"$OLD_VER\"/.version = \"$NEW_VER\"/" \
            -e "s/\"version\": \"$OLD_VER\"/\"version\": \"$NEW_VER\"/" "$f"
done

# Update Cargo.lock to match.
#
# `cargo update --workspace` re-stamps only the workspace members' versions.
# `cargo generate-lockfile` would re-resolve every dependency, which turns a
# version bump into a 2500-line dependency churn.
cargo update --workspace

# The hives are excluded from the workspace, so each has its own lockfile and
# its own copy of the version stamp. Refresh them too.
for d in hives/*/; do
  [ -f "$d/Cargo.lock" ] || continue
  (cd "$d" && cargo update --workspace)
done

# Verify no uncommitted changes aside from version bump
if ! git diff --quiet -- "${VERSIONED_FILES[@]}" Cargo.lock hives; then
  git add "${VERSIONED_FILES[@]}" Cargo.lock hives
  git commit -m "chore: bump to $NEW_VER"
fi

# Tag and push
TAG="v$NEW_VER"
git tag -a "$TAG" -m "v$NEW_VER"
git push origin main --tags
echo "Released $TAG ✓"
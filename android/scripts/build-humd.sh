#!/usr/bin/env bash
# Cross-compile the workspace `humd` daemon for aarch64-linux-android.
#
# "humd for Android" — the actual Rust daemon, built for the Android
# target (PIE ELF), not a thin thrum client. This is the ensemble
# portability problem space: the Rust workspace cross-compiles, the
# NDK supplies the C toolchain (aws-lc / ring / quinn crypto) and the
# Bionic linker. The resulting binary is copied into the Android app
# as a bundled asset so the foreground service can exec it with no
# Termux, no root.
#
# Prereqs:
#   rustup target add aarch64-linux-android
#   Android NDK (r27d) installed. Resolution order:
#     $ANDROID_NDK_HOME / $ANDROID_NDK_ROOT
#     ~/Library/Android/sdk/ndk/<latest>
#     /opt/android-ndk
#
# Usage:
#   ./android/scripts/build-humd.sh [--debug]
set -euo pipefail
cd "$(dirname "$0")/../.."          # repo root

TARGET=aarch64-linux-android
API=${ANDROID_API:-21}

# ── locate NDK ───────────────────────────────────────────────────────────────
ndk=""
for cand in "${ANDROID_NDK_HOME:-}" "${ANDROID_NDK_ROOT:-}" \
  "$HOME/Library/Android/sdk/ndk"/android-ndk-* \
  "$HOME/Library/Android/sdk/ndk"/r* \
  /opt/android-ndk; do
  if [ -n "$cand" ] && [ -d "$cand/toolchains/llvm/prebuilt" ]; then
    ndk="$cand"; break
  fi
done
if [ -z "$ndk" ]; then
  echo "build-humd: no NDK found. Install it (brew install --cask android-commandlinetools; sdkmanager 'ndk;27d') or set ANDROID_NDK_HOME." >&2
  exit 2
fi

# prebuilt host triplet: darwin-x86_64 on macOS Intel, darwin-aarch64 on AS
host="$(ls "$ndk/toolchains/llvm/prebuilt" | head -1)"
tc="$ndk/toolchains/llvm/prebuilt/$host/bin"
clang="$tc/${TARGET}${API}-clang"
if [ ! -x "$clang" ]; then
  echo "build-humd: toolchain missing $clang" >&2
  exit 2
fi

echo "ndk:      $ndk"
echo "target:   $TARGET (API $API)"
echo "clang:    $clang"

# ── cargo linker + C compiler wiring ─────────────────────────────────────────
# The workspace's C deps (aws-lc-sys, ring) need a cross clang; the final
# link needs the Bionic linker. We write a repo-local .cargo/config.toml
# (gitignored) so `cargo build --target aarch64-linux-android` just works.
mkdir -p .cargo
cat > .cargo/config.toml <<EOF
[target.${TARGET}]
linker = "${clang}"
EOF

# bash can't `export` a name containing a hyphen; cargo also honors the
# underscore form (it maps `_` -> `-`), so use CC_aarch64_linux_android etc.
export "CC_${TARGET//-/_}"="$clang"
export "AR_${TARGET//-/_}"="$tc/llvm-ar"

# ── build ────────────────────────────────────────────────────────────────────
mode="release"
[ "${1:-}" = "--debug" ] && mode="debug"
echo "building humd ($mode)..."
cargo build -p humd --target "$TARGET" ${mode/release/--release}

bin="target/$TARGET/${mode/release/release}/humd"
out="android/app/src/main/assets/humd"
mkdir -p "$(dirname "$out")"
cp -f "$bin" "$out"
chmod 755 "$out"

echo "→ bundled to $out ($(du -h "$out" | cut -f1))"
file "$out"
echo "build-humd: done"

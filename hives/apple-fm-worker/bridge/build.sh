#!/usr/bin/env bash
# Build the apple-fm-worker bridge — Apple's on-device 3B (FoundationModels)
# wrapped as a one-shot NDJSON subprocess. The ONLY Apple-specific artifact.
#
# Requires: macOS 26 + Apple Intelligence enabled, and the Command Line Tools
# swiftc (no full Xcode needed — the framework resolves via the CLT SDK).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="${1:-$HERE/gen}"
# arm64e is the native slice on Apple Silicon; arm64e-apple-macos26.0 is the
# minimum target FoundationModels ships at.
swiftc -parse-as-library -target arm64e-apple-macos26.0 \
    "$HERE/gen.swift" -O -o "$OUT"
echo "built $OUT"

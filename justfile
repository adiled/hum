# hum — justfile. Cross-cutting orchestration over a Rust core + per-
# subproject TS manifests. `just <recipe>` runs a recipe. `just` alone
# lists them. No root package.json, no pnpm workspace.

# default: list recipes
default:
    @just --list

# Rust binaries (release humd)
build:
    cargo build --release -p humd

# Build every TS bee standalone (no workspace coordination).
bees:
    #!/usr/bin/env bash
    set -euo pipefail
    for n in hives/*/; do
      if [ -f "$n/package.json" ]; then
        echo "→ $n"
        (cd "$n" && pnpm install --silent >/dev/null 2>&1 && pnpm run build --silent)
      fi
    done

# Everything (Rust + TS bees)
build-all: build bees

# Rust tests across all crates
test:
    cargo test --workspace

# Recipe tests (vitest) — opencode integration
test-recipes:
    #!/usr/bin/env bash
    if [ -f recipes/opencode/tests/package.json ]; then
      cd recipes/opencode/tests && pnpm install --silent >/dev/null 2>&1 && pnpm test
    else
      echo "no recipes/opencode/tests/package.json"
    fi

# Fast Rust subset: sim narratives only
sim:
    cargo test -p sim

# Workspace type check (no test compile)
check:
    cargo check --workspace

# The repo is not rustfmt-clean — running fmt --all would rewrite ~380
# hunks across every file. fmt reports; fmt-file formats one file.
fmt:
    cargo fmt --all -- --check

fmt-file file:
    rustfmt --edition 2024 {{file}}

# The kernel speaks in code: no `//` and no `///` in the kernel's own
# Rust. `//!` is the module header and is the only comment allowed.
# The hives are exempt — each is a separate workspace, and a kind any
# third party may write in any language.
speech:
    python3 tools/speech.py

# Clippy over the crates CI gates on. Same scope, same flags as
# .github/workflows/wire.yml — if this passes, CI passes.
lint: speech
    cargo clippy -p ensemble -p hum-paths --all-targets -- -D warnings -A clippy::type_complexity -A clippy::double_must_use

# Clippy over everything. The daemon and the drone still carry dead
# code, so this reports the dead_code findings too.
lint-all:
    cargo clippy --workspace --all-targets -- -D warnings -A clippy::type_complexity -A clippy::double_must_use

# Docs site (Astro + Starlight)
site:
    cd site && pnpm install --no-audit --no-fund && pnpm run build

site-dev:
    cd site && pnpm install --no-audit --no-fund && pnpm run dev

# Paradigm 2: full installer — humd binary, systemd unit, hum.json
install:
    ./install

# Local-dev redeploy (env-specific glue, gitignored)
dev:
    ./dev/deploy

# Recipe: bring up hum + opencode end-to-end
recipe-opencode:
    ./recipes/opencode/install

clean:
    cargo clean
    rm -rf site/dist site/.astro site/src/content/docs
    @for n in hives/*/dist; do rm -rf "$n" 2>/dev/null || true; done

purge: clean
    ./install purge

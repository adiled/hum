# hum — justfile. Cross-cutting orchestration over a Rust core + per-
# subproject TS manifests. `just <recipe>` runs a recipe. `just` alone
# lists them. No root package.json, no pnpm workspace.

# default: list recipes
default:
    @just --list

# Rust binaries (release humd)
build:
    cargo build --release -p humd

# Build every TS nestling standalone (no workspace coordination).
nestlings:
    #!/usr/bin/env bash
    set -euo pipefail
    for n in openai-server anthropic-server ollama-server vercel-ai; do
      if [ -f "nestlings/$n/package.json" ]; then
        echo "→ nestlings/$n"
        (cd "nestlings/$n" && pnpm install --silent >/dev/null 2>&1 && pnpm run build --silent)
      fi
    done

# Everything (Rust + TS nestlings)
build-all: build nestlings

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

fmt:
    cargo fmt --all

lint:
    cargo clippy --workspace -- -D warnings

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
    @for n in nestlings/*/dist; do rm -rf "$n" 2>/dev/null || true; done

purge: clean
    ./install purge

# hum — top-level Makefile. Thin convenience over cargo / npm / installer.
#
# Targets that matter:
#   make           — alias for `make build`
#   make build     — release humd binary at target/release/humd
#   make test      — cargo test --workspace (157 tests as of 0.3)
#   make sim       — sim narratives only (fast subset, no real wire)
#   make site      — build the GitHub Pages site (Astro + Starlight)
#   make site-dev  — local site preview on http://localhost:4321/hum/
#   make install   — run the installer (./install) — builds + systemd + start
#   make dev       — local dev-loop deploy (rsync + restart on $HUM_DEV_USER)
#   make clean     — drop cargo target/ and site build artifacts
#   make purge     — clean + `./install purge` (wipes state — be sure)
#
# Variables you can override:
#   CARGO       cargo binary (default: cargo)
#   NPM         npm binary  (default: npm)
#   SITE_DIR    site folder (default: site)
#
CARGO    ?= cargo
NPM      ?= npm
SITE_DIR ?= site
HUMD_BIN  := target/release/humd

.PHONY: all build test sim site site-dev install dev clean purge fmt lint check

all: build

# ── build ─────────────────────────────────────────────────────────────────────

build:
	$(CARGO) build --release -p humd

$(HUMD_BIN): build

# ── tests ─────────────────────────────────────────────────────────────────────

test:
	$(CARGO) test --workspace

# Fast subset: only the sim narratives over InMemoryEndpoint. No real wire,
# no claude binary needed, no disk-heavy linking against rustls/iroh.
sim:
	$(CARGO) test -p sim

# Workspace check without test compile — quick sanity.
check:
	$(CARGO) check --workspace

# ── code quality (best-effort, won't fail the loop) ───────────────────────────

fmt:
	$(CARGO) fmt --all

lint:
	$(CARGO) clippy --workspace -- -D warnings

# ── docs site (GitHub Pages) ──────────────────────────────────────────────────

site:
	cd $(SITE_DIR) && $(NPM) install --no-audit --no-fund && $(NPM) run build

site-dev:
	cd $(SITE_DIR) && $(NPM) install --no-audit --no-fund && $(NPM) run dev

# ── install / dev-loop ────────────────────────────────────────────────────────

install: build
	./install

dev:
	./dev

# ── housekeeping ──────────────────────────────────────────────────────────────

clean:
	$(CARGO) clean
	rm -rf $(SITE_DIR)/dist $(SITE_DIR)/.astro $(SITE_DIR)/src/content/docs

purge: clean
	./install purge

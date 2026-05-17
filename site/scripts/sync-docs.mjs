#!/usr/bin/env node
// sync-docs.mjs — pull README.md and selected docs from the repo into
// site/src/content/docs/, normalizing relative links, prepending a
// Starlight frontmatter block. Run as `npm run sync` before build.

import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT  = path.resolve(__dirname, "../../");
const DEST_ROOT  = path.resolve(__dirname, "../src/content/docs/");

/**
 * One entry = one page on the site.
 *   src        — repo-relative source markdown
 *   slug       — destination path under src/content/docs/ (no ext)
 *   title      — Starlight page title
 *   description — short tagline shown in TOC + meta
 */
// README.md files plus a small explicit allowlist of other reference
// docs (WIRE spec, etc.). Other markdown in the repo (scenarios/*.md,
// docs/*.md) is local-only — included earlier, generated 404s on the
// deployed site because subpages weren't routed cleanly. The on-mesh
// model means nestlings live in their own repos anyway; this site
// reflects what's reference-quality in THIS repo.
const PAGES = [
  { src: "README.md",                       slug: "index",                  title: "hum",                      description: "The only AI stack nestled on a biodiverse agentic kernel framework." },
  { src: "ensemble/README.md",              slug: "ensemble/index",         title: "Ensemble — the mesh",      description: "Cross-humd routing, discovery, gossip, Kademlia. Where humds find each other." },
  { src: "thrum/WIRE.md",                   slug: "thrum/wire",             title: "Wire spec",                description: "Language-neutral protocol spec — implementable in any language with NDJSON + Unix sockets." },
  { src: "thrum-core/README.md",            slug: "thrum-core/index",       title: "thrum-core (Rust)",        description: "Wire-protocol primitives for Rust nestlings." },
  { src: "thrum/README.md",                 slug: "thrum/index",            title: "thrum (TS)",               description: "Wire-protocol primitives for TS / JS nestlings." },
  { src: "clients/python/README.md",        slug: "clients/python",         title: "thrum (Python)",           description: "Python reference client — generated from the same Rust source of truth." },
  { src: "clients/go/README.md",            slug: "clients/go",             title: "thrum (Go)",               description: "Go reference client — generated from the same Rust source of truth." },
  { src: "nestlings/README.md",             slug: "nestlings/index",        title: "Nestlings — the typology", description: "Statefulness × richness × wire shape. How to build a new nestling." },
  { src: "nestlings/opencode/README.md",    slug: "nestlings/opencode",     title: "opencode",                 description: "Rich, stateful nestling for OpenCode." },
  { src: "nestlings/openai-server/README.md",    slug: "nestlings/openai-server",    title: "openai-server",    description: "OpenAI-compatible HTTP/SSE surface for hum." },
  { src: "nestlings/anthropic-server/README.md", slug: "nestlings/anthropic-server", title: "anthropic-server", description: "Anthropic Messages API surface for hum — drop-in for @anthropic-ai/sdk." },
  { src: "nestlings/vercel-ai/README.md",   slug: "nestlings/vercel-ai",    title: "vercel-ai",                description: "Vercel AI SDK provider — drive hum from any `ai` SDK caller." },
  { src: "nestlings/grpc/README.md",        slug: "nestlings/grpc",         title: "grpc (Rust)",              description: "Transport-only bidi gRPC bridge — every chi flows through." },
  { src: "nestlings/paid-oracle/README.md", slug: "nestlings/paid-oracle",  title: "paid-oracle (Rust)",       description: "x402-style paid oracle — one price per USDC payment, on-chain verified." },
  { src: "nestlings/ollama-server/README.md", slug: "nestlings/ollama-server", title: "ollama-server (Rust)",  description: "Ollama-compatible HTTP surface for hum — drop-in for local Ollama clients." },
  { src: "nestlings/twilio-sms/README.md",  slug: "nestlings/twilio-sms",   title: "twilio-sms (Go)",          description: "Twilio SMS webhook nestling — text messages in, hum replies out." },
  { src: "nestlings/gsm-modem/README.md",   slug: "nestlings/gsm-modem",    title: "gsm-modem (Rust)",         description: "GSM modem nestling — SMS over a serial-attached cellular modem." },
  { src: "contracts/README.md",             slug: "contracts/index",        title: "Contracts (Solidity)",     description: "On-chain commitments for hum — HumdRegistry, escrow primitives, future settlement." },
  { src: "scenarios/README.md",             slug: "scenarios/index",        title: "Scenarios",                description: "Five narratives, each mapped 1:1 to a sim test." },
];

function escapeYaml(s) {
  return String(s).replace(/"/g, '\\"');
}

function frontmatter({ title, description }) {
  return [
    "---",
    `title: "${escapeYaml(title)}"`,
    `description: "${escapeYaml(description)}"`,
    "---",
    "",
  ].join("\n");
}

/**
 * Rewrite repo-relative links so they resolve under the site:
 *   ../ensemble/      → /ensemble/
 *   ./README.md       → ./
 *   ./foo.md          → ./foo/
 *   /root/hum/foo.md  → /foo/ (rare absolute repo refs)
 *
 * Only touches markdown link/image syntax, leaves code blocks alone.
 */
function rewriteLinks(body, srcRelDir) {
  let inFence = false;
  return body
    .split("\n")
    .map((line) => {
      if (/^\s*```/.test(line)) {
        inFence = !inFence;
        return line;
      }
      if (inFence) return line;
      return line.replace(/(\[[^\]]+\])\(([^)]+)\)/g, (m, label, target) => {
        // Skip external URLs and anchors.
        if (/^[a-z][a-z0-9+.-]*:/i.test(target)) return m;
        if (target.startsWith("#")) return m;
        // Drop "/root/hum/" prefix if present.
        let t = target.replace(/^\/root\/hum\//, "/");
        // README.md → directory root
        t = t.replace(/(^|\/)README\.md(\b|$)/, "$1");
        // Other .md → /slug/
        t = t.replace(/\.md(\b|$)/, "/");
        // Trim leading "./"
        t = t.replace(/^\.\//, "");
        return `${label}(${t})`;
      });
    })
    .join("\n");
}

async function copyOne(page) {
  const src  = path.join(REPO_ROOT, page.src);
  const dest = path.join(DEST_ROOT, `${page.slug}.md`);
  await fs.mkdir(path.dirname(dest), { recursive: true });

  let body;
  try {
    body = await fs.readFile(src, "utf8");
  } catch (e) {
    console.warn(`[sync-docs] missing: ${page.src} — skipping (${e.code})`);
    return;
  }

  // Strip the first H1 — Starlight renders title from frontmatter.
  body = body.replace(/^#\s+[^\n]+\n+/, "");

  body = rewriteLinks(body, path.dirname(page.src));
  await fs.writeFile(dest, frontmatter(page) + body, "utf8");
  console.log(`[sync-docs] ${page.src} → ${path.relative(REPO_ROOT, dest)}`);
}

async function writeVocabulary() {
  // Hand-curated glossary — pulls from CLAUDE.md style vocab, adapted.
  const dest = path.join(DEST_ROOT, "vocabulary.md");
  await fs.mkdir(path.dirname(dest), { recursive: true });
  const body = `${frontmatter({
    title: "Vocabulary",
    description: "The biodiverse register hum thinks in. Names carry meaning beyond function.",
  })}
This page is a quick lookup of every word that appears in the hum
codebase as a load-bearing term. Names are chosen so they *feel*
like the thing — readers and writers share the same mental model.

## Wire

- **thrum** — the bidirectional NDJSON socket between humd and any nestler.
- **tone** — one message frame on the thrum. Envelope (chi/rid/sid/…) plus body.
- **chi** — the tone's discriminator. \`prompt\`, \`chunk\`, \`finish\`, \`gossip-publish\`, …
- **sigil** — content-addressable session pairing hash. Stable across reconnects.
- **wane** — Lamport-clock per sigil. Increments on every state mutation.
- **dusk** — absolute ms expiry on a tone. Past dusk, drop.

## Daemon

- **humd** — the daemon process. One per machine install.
- **HumdId** — sha256 of the humd's Ed25519 public key.
- **hum** — one conversation. Has a hum_id, lives on a humd.
- **nest** — a class of model harness (claude-cli, claude-repl, future kinds).
- **roost** — one live nest process (one Claude subprocess, say).
- **perch** — the strategy that spawns a roost.
- **brood** — the state machine that walks a roost from cold to ready (PTY-only).

## Conversation

- **petal** — one unit of content (text, image, tool_use, tool_result).
- **petal-cell** — a nestler's view of one petal in its own conversation graph.
- **bloom** — one turn of conversation. Starts with a prompt, ends with a finish.
- **wilt** — close the bloom.
- **buds** — buffered tool events not yet committed.
- **shed** — flush the buds.
- **tendril** — brokered tool call. Reaches across the wire for execution.
- **sap** — accumulated tool input being assembled.

## Ensemble

- **ensemble** — the mesh of cooperating humds.
- **nestling** — the kind a nestler conforms to. The OC plugin is one; the
  market-maker agent is another.
- **nestler** — one running instance of a nestling.
- **nestled** — a nestler post-handshake. After it has nestled.

## Observation

- **drone** — the sentinel watching every tone. Self-governance + drift detection.
- **drift** — timing rings per bloom. p50/p95 across humds.
- **penny** — lifetime counters. Token swaps, tool executions, etc.
`;
  await fs.writeFile(dest, body, "utf8");
  console.log(`[sync-docs] wrote vocabulary.md`);
}

await fs.rm(DEST_ROOT, { recursive: true, force: true });
await fs.mkdir(DEST_ROOT, { recursive: true });
for (const page of PAGES) await copyOne(page);
await writeVocabulary();
console.log(`[sync-docs] done — ${PAGES.length + 1} pages.`);

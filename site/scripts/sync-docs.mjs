#!/usr/bin/env node
// sync-docs.mjs — walk the repo, copy every publishable markdown into
// site/src/content/docs/, rewriting intra-repo links to absolute site
// URLs. Run as `npm run sync` before build.
//
// Source files are expected to carry their own Starlight frontmatter
// (title + optional description). No frontmatter is generated here.
//
// What's publishable:
//   - Every `*.md` outside any `docs/` directory.
//   - Excluded by name at any depth: AGENTS.md, CLAUDE.md.
//   - Skipped directories: .git, .claude, node_modules, target, dist, site.

import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, "../../");
const DEST_ROOT = path.resolve(__dirname, "../src/content/docs/");

// Site base path. When deploying to https://<owner>.github.io/hum/,
// every rewritten link needs `/hum/` prepended — Astro/Starlight
// auto-prefixes its own anchors but not markdown links in body.
const SITE_BASE = (process.env.HUM_SITE_BASE ?? "/hum/").replace(/\/?$/, "/");

const SKIP_DIRS = new Set([
  ".git", ".claude", "node_modules", "target", "dist", "docs", "site",
]);
const SKIP_FILES = new Set(["AGENTS.md", "CLAUDE.md"]);

function shouldDescend(name) {
  return !SKIP_DIRS.has(name) && !name.startsWith(".");
}

async function* walk(dir = REPO_ROOT, rel = "") {
  const entries = await fs.readdir(dir, { withFileTypes: true });
  for (const e of entries) {
    if (e.isDirectory()) {
      if (!shouldDescend(e.name)) continue;
      yield* walk(path.join(dir, e.name), path.join(rel, e.name));
    } else if (e.isFile() && e.name.endsWith(".md") && !SKIP_FILES.has(e.name)) {
      yield path.join(rel, e.name);
    }
  }
}

/**
 * Slug for a repo-relative md path.
 *
 *   README.md                    → "index"
 *   ensemble/README.md           → "ensemble/index"
 *   thrum/WIRE.md                → "thrum/wire"
 *   VOCABULARY.md                → "vocabulary"
 *   reports/compat-v0.10.3.md    → "reports/compat-v0.10.3"
 */
function deriveSlug(rel) {
  const dir = path.dirname(rel);
  const base = path.basename(rel, ".md");
  if (base === "README") {
    return dir === "." ? "index" : `${dir}/index`;
  }
  return dir === "." ? base.toLowerCase() : `${dir}/${base.toLowerCase()}`;
}

/** Site URL for a slug, base-prefixed.
 *
 *   index           → "/hum/"
 *   foo/index       → "/hum/foo/"
 *   foo/bar         → "/hum/foo/bar/"
 *
 * The leading slash + trailing slash conventions match Starlight's
 * trailingSlash="always" config. The base is included so markdown
 * links written into the body resolve correctly on the deployed site.
 */
function slugToUrl(slug) {
  const trimmed = SITE_BASE.replace(/\/$/, "");
  if (slug === "index") return SITE_BASE;
  if (slug.endsWith("/index")) return `${trimmed}/${slug.slice(0, -"index".length)}`;
  return `${trimmed}/${slug}/`;
}

async function buildPageMap() {
  const map = new Map(); // repoRelativePath → "/site/url/"
  for await (const rel of walk()) {
    map.set(rel, slugToUrl(deriveSlug(rel)));
  }
  return map;
}

/**
 * Rewrite intra-repo markdown links to absolute site URLs. External /
 * anchor / non-repo links pass through. Code fences are left alone.
 */
function rewriteLinks(body, srcRel, pageMap) {
  const srcDir = path.dirname(srcRel);
  let inFence = false;
  return body.split("\n").map((line) => {
    if (/^\s*```/.test(line)) {
      inFence = !inFence;
      return line;
    }
    if (inFence) return line;
    return line.replace(/(\[[^\]]+\])\(([^)]+)\)/g, (m, label, target) => {
      if (/^[a-z][a-z0-9+.-]*:/i.test(target)) return m; // external scheme
      if (target.startsWith("#")) return m;             // anchor
      if (target.startsWith("/")) return m;             // already absolute

      const cleaned = target.replace(/^\.\//, "");
      const resolved = path.posix.normalize(
        srcDir === "." ? cleaned : `${srcDir}/${cleaned}`,
      );

      if (pageMap.has(resolved)) {
        return `${label}(${pageMap.get(resolved)})`;
      }
      const indexAttempt = `${resolved.replace(/\/$/, "")}/README.md`;
      if (pageMap.has(indexAttempt)) {
        return `${label}(${pageMap.get(indexAttempt)})`;
      }
      // Source-tree pointer (Rust file, etc) — leave as-is; the reader
      // will resolve it on GitHub.
      return m;
    });
  }).join("\n");
}

/**
 * Source files carry their own frontmatter and (typically) an H1. The
 * H1 must be stripped before writing — Starlight renders the title
 * from frontmatter and a duplicate H1 in the body is double-render.
 *
 * Splits `--- ... ---` frontmatter, leaves it untouched, strips only
 * the first H1 from the body below.
 */
function stripBodyH1(content) {
  const fmEnd = content.startsWith("---\n")
    ? content.indexOf("\n---\n", 4)
    : -1;
  if (fmEnd < 0) {
    return content.replace(/^#\s+[^\n]+\n+/, "");
  }
  const head = content.slice(0, fmEnd + "\n---\n".length);
  const body = content.slice(fmEnd + "\n---\n".length);
  return head + body.replace(/^\s*#\s+[^\n]+\n+/, "");
}

async function syncOne(rel, pageMap) {
  const src = path.join(REPO_ROOT, rel);
  let content;
  try {
    content = await fs.readFile(src, "utf8");
  } catch (e) {
    console.warn(`[sync-docs] read failed ${rel}: ${e.code}`);
    return null;
  }
  if (!content.startsWith("---\n")) {
    console.warn(`[sync-docs] no frontmatter at ${rel} — page will use the filename as title`);
  }
  content = stripBodyH1(content);
  content = rewriteLinks(content, rel, pageMap);

  const slug = deriveSlug(rel);
  const dest = path.join(DEST_ROOT, `${slug}.md`);
  await fs.mkdir(path.dirname(dest), { recursive: true });
  await fs.writeFile(dest, content, "utf8");
  return slug;
}

async function main() {
  await fs.rm(DEST_ROOT, { recursive: true, force: true });
  await fs.mkdir(DEST_ROOT, { recursive: true });

  const pageMap = await buildPageMap();
  let count = 0;
  for await (const rel of walk()) {
    const slug = await syncOne(rel, pageMap);
    if (slug) {
      console.log(`[sync-docs] ${rel} → ${slug}.md`);
      count += 1;
    }
  }
  console.log(`[sync-docs] done — ${count} pages.`);
}

await main();

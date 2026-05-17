// @ts-check
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const CONTENT_DOCS = path.resolve(__dirname, "src/content/docs");

// Site path. When deploying to https://<owner>.github.io/hum/, the
// `base` must be "/hum/". Override with HUM_SITE_BASE in CI for
// custom domains.
const base = process.env.HUM_SITE_BASE ?? "/hum/";
const site = process.env.HUM_SITE_URL ?? "https://adiled.github.io";

/**
 * Walk the synced content directory and produce a sidebar tree that
 * mirrors the file layout. Each `.md` file becomes a `{ slug }` link;
 * each directory becomes a group whose `items[]` are the directory's
 * children. The directory's own `index.md` (if present) is hoisted as
 * the group's `link`, and its title is reused as the group label.
 *
 * This sidebar runs at config-load time, so `npm run sync` must have
 * populated `src/content/docs/` first. The `predev` / `prebuild`
 * scripts in `package.json` enforce that.
 */
function buildSidebar(dir = CONTENT_DOCS, baseSlug = "") {
  if (!fs.existsSync(dir)) return [];
  const items = [];
  const entries = fs.readdirSync(dir, { withFileTypes: true }).sort((a, b) => {
    if (a.name === "index.md") return -1;
    if (b.name === "index.md") return 1;
    return a.name.localeCompare(b.name);
  });

  for (const e of entries) {
    if (e.isDirectory()) {
      const subDir = path.join(dir, e.name);
      const subSlug = baseSlug ? `${baseSlug}/${e.name}` : e.name;
      const indexPath = path.join(subDir, "index.md");
      const hasIndex = fs.existsSync(indexPath);
      // Pull child files first (everything except index.md, which gets
      // hoisted as the group's first "Overview" entry below).
      const childItems = buildSidebar(subDir, subSlug);
      const groupLabel = hasIndex
        ? titleFromFrontmatter(indexPath, e.name)
        : e.name;

      if (hasIndex && childItems.length === 0) {
        // Directory with only an index page — collapse to a single link.
        items.push({ label: groupLabel, link: `/${subSlug}/` });
        continue;
      }

      const groupItems = hasIndex
        ? [{ label: "Overview", link: `/${subSlug}/` }, ...childItems]
        : childItems;
      items.push({ label: groupLabel, items: groupItems, collapsed: false });
    } else if (e.name.endsWith(".md")) {
      if (e.name === "index.md") {
        // The parent call hoists index.md as the group's "Overview"
        // link; skip it here so we don't double-emit.
        continue;
      }
      const fullPath = path.join(dir, e.name);
      const slug = baseSlug
        ? `${baseSlug}/${e.name.replace(/\.md$/, "")}`
        : e.name.replace(/\.md$/, "");
      items.push({
        label: titleFromFrontmatter(fullPath, e.name.replace(/\.md$/, "")),
        link: `/${slug}/`,
      });
    }
  }
  return items;
}

/** Parse `title:` from a Starlight frontmatter block. Fallback: `fallback`. */
function titleFromFrontmatter(filePath, fallback) {
  try {
    const head = fs.readFileSync(filePath, "utf8").slice(0, 2048);
    if (!head.startsWith("---")) return fallback;
    const end = head.indexOf("\n---", 3);
    if (end < 0) return fallback;
    const fm = head.slice(3, end);
    const m = fm.match(/^title:\s*"?([^"\n]+?)"?\s*$/m);
    return m ? m[1].trim() : fallback;
  } catch {
    return fallback;
  }
}

const treeSidebar = buildSidebar();
const rootIndex = fs.existsSync(path.join(CONTENT_DOCS, "index.md"));

export default defineConfig({
  site,
  base,
  trailingSlash: "always",
  integrations: [
    starlight({
      title: "hum",
      description: "The only AI stack nestled on a biodiverse agentic kernel framework.",
      logo: { src: "./src/assets/hum-glyph.svg" },
      social: { github: "https://github.com/adiled/hum" },
      customCss: ["./src/styles/spacecraft.css"],
      sidebar: [
        ...(rootIndex ? [{ label: "Overview", link: "/" }] : []),
        ...treeSidebar,
      ],
    }),
  ],
});

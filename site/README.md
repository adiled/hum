---
title: "site"
---

# site

GitHub Pages source for [hum](https://github.com/adiled/hum). Built with
Astro + Starlight, spacecraft-themed CSS.

## How it works

`scripts/sync-docs.mjs` walks the repo and takes every markdown file
outside `docs/`, `vendor/` and build output, except `AGENTS.md` and
`CLAUDE.md`. It normalizes their links, prepends a Starlight frontmatter
block where a file has none, and writes them into `src/content/docs/`. The Astro build then renders them with the
spacecraft theme.

The sync runs automatically on every `npm run dev` and `npm run build`,
so editing any of the source `README.md` files in the repo is enough —
the site picks up the changes on the next build.

## Local dev

```bash
cd site
npm install
npm run dev          # http://localhost:4321/hum/
```

## Deploy

Pushed to `main` triggers `.github/workflows/pages.yml` which builds
and publishes to `https://adiled.github.io/hum/`.

## Adding a page

Edit `scripts/sync-docs.mjs` — append a row to the `PAGES` array with
the source path + slug + title + description. Add a matching entry to
the `sidebar` array in `astro.config.mjs`.

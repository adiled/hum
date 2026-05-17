// @ts-check
import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";

// Site path. When deploying to https://<owner>.github.io/hum/, the
// `base` must be "/hum/". Override with HUM_SITE_BASE in CI for
// custom domains.
const base = process.env.HUM_SITE_BASE ?? "/hum/";
const site = process.env.HUM_SITE_URL ?? "https://adiled.github.io";

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
      components: {
        // Use Starlight's built-in head; spacecraft CSS handles the rest.
      },
      // Sidebar is generated from the synced content tree. Add a
      // README.md anywhere in the repo (outside `docs/` and the by-name
      // exclusions) and it shows up here automatically. See
      // `site/scripts/sync-docs.mjs` for the discovery rules.
      sidebar: [
        { label: "Overview",   link: "/" },
        { label: "Vocabulary", link: "/vocabulary/" },
        { label: "Docs", autogenerate: { directory: "." }, collapsed: false },
      ],
    }),
  ],
});

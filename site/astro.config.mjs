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
      sidebar: [
        { label: "Overview",   link: "/" },
        { label: "Vocabulary", link: "/vocabulary/" },
        {
          label: "Architecture",
          items: [
            { label: "Ensemble (mesh)", link: "/ensemble/" },
          ],
        },
        {
          label: "Build a nestling",
          items: [
            { label: "thrum-core (Rust)", link: "/thrum-core/" },
            { label: "thrum (TS)",        link: "/thrum/" },
          ],
        },
        {
          label: "Nestlings",
          collapsed: false,
          items: [
            { label: "Typology", link: "/nestlings/" },
            {
              label: "Reference",
              collapsed: false,
              items: [
                { label: "opencode",          link: "/nestlings/opencode/" },
                { label: "openai-server",     link: "/nestlings/openai-server/" },
                { label: "vercel-ai",         link: "/nestlings/vercel-ai/" },
                { label: "grpc (Rust)",       link: "/nestlings/grpc/" },
                { label: "paid-oracle (Rust)", link: "/nestlings/paid-oracle/" },
              ],
            },
          ],
        },
        { label: "Scenarios", link: "/scenarios/" },
      ],
    }),
  ],
});

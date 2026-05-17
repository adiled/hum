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
            { label: "Wire spec", link: "/thrum/wire/" },
            {
              label: "Client libraries",
              collapsed: false,
              items: [
                { label: "thrum-core (Rust)", link: "/thrum-core/" },
                { label: "thrum (TS)",        link: "/thrum/" },
                { label: "thrum (Python)",    link: "/clients/python/" },
                { label: "thrum (Go)",        link: "/clients/go/" },
              ],
            },
          ],
        },
        {
          label: "On-chain",
          items: [
            { label: "Contracts (Solidity)", link: "/contracts/" },
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
                { label: "opencode",            link: "/nestlings/opencode/" },
                { label: "openai-server",       link: "/nestlings/openai-server/" },
                { label: "anthropic-server",    link: "/nestlings/anthropic-server/" },
                { label: "vercel-ai",           link: "/nestlings/vercel-ai/" },
                { label: "grpc (Rust)",         link: "/nestlings/grpc/" },
                { label: "paid-oracle (Rust)",  link: "/nestlings/paid-oracle/" },
                { label: "ollama-server (Rust)", link: "/nestlings/ollama-server/" },
                { label: "gsm-modem (Rust)",    link: "/nestlings/gsm-modem/" },
                { label: "twilio-sms (Go)",     link: "/nestlings/twilio-sms/" },
              ],
            },
          ],
        },
        { label: "Scenarios", link: "/scenarios/" },
      ],
    }),
  ],
});

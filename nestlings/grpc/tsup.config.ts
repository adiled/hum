import { defineConfig } from "tsup";

export default defineConfig({
  entry: { index: "src/index.ts" },
  format: "esm",
  platform: "node",
  target: "node18",
  banner: { js: "#!/usr/bin/env node" },
  loader: { ".proto": "copy" },
  external: ["@grpc/grpc-js", "@grpc/proto-loader"],
});

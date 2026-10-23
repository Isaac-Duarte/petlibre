import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vitest/config";

// `dist/` is embedded into petlibre-bench (`--features ui`), so the build is
// self-contained: relative asset paths, no CDN, fonts bundled.
// `pnpm dev` proxies the API to a bench page already serving on 8780.
export default defineConfig({
  plugins: [svelte()],
  base: "./",
  build: {
    outDir: "dist",
    emptyOutDir: true,
    assetsInlineLimit: 0,
    target: "es2022",
  },
  server: {
    proxy: {
      "/api": { target: process.env.BENCH_URL ?? "http://127.0.0.1:8780", changeOrigin: false },
    },
  },
  test: {
    include: ["src/**/*.test.ts"],
  },
});

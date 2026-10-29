import { sveltekit } from "@sveltejs/kit/vite";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [await sveltekit()],
  test: { include: ["src/**/*.test.ts"] },
  // docs/ sits outside the web workspace; mermaid.test.ts reads it from jsdom.
  server: { fs: { allow: ["../../../.."] } },
});

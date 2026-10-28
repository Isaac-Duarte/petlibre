import adapter from "@sveltejs/adapter-static";
import { sveltekit } from "@sveltejs/kit/vite";
import { defineConfig } from "vite";

// docs.well-fed.dev renders the repository's docs/ at build time
// (docs/web/docs-site.md). Every page is prerendered, and the crawl fails on
// any internal link or #anchor that doesn't resolve, so a broken link in docs/
// breaks the build rather than the site. Nothing is fetched from another
// origin at runtime.
export default defineConfig({
  plugins: [
    await sveltekit({
      adapter: adapter({ strict: true }),
      prerender: { handleHttpError: "fail", handleMissingId: "fail" },
      // Prerendered pages carry this policy as a <meta> tag, with a hash for
      // each inline script SvelteKit writes. static/_headers holds the rest.
      csp: {
        mode: "hash",
        directives: { "script-src": ["self", "wasm-unsafe-eval"] },
      },
    }),
  ],
  server: {
    // docs/ and assets/brand/ sit outside the web workspace.
    fs: { allow: ["../../../.."] },
  },
  build: {
    assetsInlineLimit: 0,
    target: "es2022",
  },
});

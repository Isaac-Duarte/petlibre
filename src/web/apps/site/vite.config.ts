import adapter from "@sveltejs/adapter-static";
import { sveltekit } from "@sveltejs/kit/vite";
import { defineConfig } from "vite";

// well-fed.dev is one prerendered page served as static files by Cloudflare
// Pages (docs/web/site.md). Nothing is fetched at runtime: fonts, images and
// styles are bundled, so the page makes no request off its own origin.
export default defineConfig({
  plugins: [
    await sveltekit({
      adapter: adapter({ strict: true }),
      prerender: { handleHttpError: "fail", handleMissingId: "fail" },
    }),
  ],
  build: {
    assetsInlineLimit: 0,
    target: "es2022",
  },
});

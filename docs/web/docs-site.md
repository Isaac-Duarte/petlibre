# docs.well-fed.dev

This page is for maintainers of the docs site. The site is every Markdown file under `docs/`, rendered by the SvelteKit app in `src/web/apps/docs` and served as static files by a Cloudflare Worker.

The Markdown stays plain GitHub-flavored Markdown that reads the same on GitHub. The site reads it at build time and never changes it, so you write docs exactly as before: the [writing guide](../development/writing.md) still applies.

## What it is

| Area | Choice |
| --- | --- |
| Build | SvelteKit 3 with `adapter-static`; every page is prerendered from `docs/` at build time |
| Markdown | unified (remark, GFM, rehype) in `src/lib/server/content.ts`, not mdsvex: the docs are never compiled as Svelte, so a `{` or `<` in prose is just text |
| Links | A link to another doc becomes its route; a link anywhere else in the repository becomes that file on GitHub. The prerender crawl fails the build on any link or `#anchor` that doesn't resolve |
| Code | Shiki at build time, one theme whose colors are the token roles, so code follows light and dark |
| Diagrams | A ```` ```mermaid ```` block naming an SVG (`%% svg: name.svg`) becomes that SVG, inlined so `docs.css` colors its `.d-` classes from the tokens. GitHub draws the Mermaid. `cargo xtask check-repository` holds each pair to its hashes in `docs/diagrams.lock` ([writing guide](../development/writing.md#diagrams)) |
| Overview \| Details | A page whose overview ends with `<!-- details -->` gets a switch that hides the rest. The choice holds on every page, is kept in `localStorage`, and is set before first paint by one inline script, whose hash `vite.config.ts` adds to the CSP. Without JavaScript every page shows in full |
| Search | Pagefind indexes `build/` after the prerender and serves its index from `/pagefind/`, so a query never leaves the site |
| Style | The [tokens](../../assets/brand/tokens.json) and `@wellfed/ui`'s base, as on [well-fed.dev](site.md): type and hairlines, no cards or gradients. Light or dark follows the system |
| Requests | None off its own origin. Each page carries a CSP `<meta>` with a hash for every inline script, and `static/_headers` sets the rest |
| Tests | `svelte-check`, ESLint and Prettier with the workspace, Vitest checks that the sidebar lists every doc and that overviews and diagrams render, and Playwright against `vite preview` at desktop and phone width |

## Adding a page

1. Write the page under `docs/` and link it from its folder's README, as usual.
2. Add it to `src/web/apps/docs/src/lib/nav.ts` in its group. Give it a `label` if its heading is long.
3. Run the build (below). It fails if a link or anchor is broken, and the nav test fails if a doc is missing from the sidebar.

## Develop

```sh
pnpm -C src/web install
pnpm -C src/web generate
pnpm -C src/web --filter @wellfed/docs dev      # http://localhost:5173, without search
pnpm -C src/web --filter @wellfed/docs build    # writes src/web/apps/docs/build/, with the search index
pnpm -C src/web --filter @wellfed/docs e2e
```

The dev server reloads when a doc changes. Search needs the index, so it works only in a build. `pnpm -C src/web --filter @wellfed/docs preview` serves `build/` the way the Worker does, from `e2e/serve.ts`; `vite preview` would miss the index.

`make ui` and `make check-web` build, check and test the site with the other apps.

## Deploy

The settings mirror [well-fed.dev's](site.md#deploy). Cloudflare Workers Builds builds from the repository with its Git integration, and nothing in the repository holds a Cloudflare credential. The Worker has no script: [`wrangler.jsonc`](../../src/web/apps/docs/wrangler.jsonc) serves the build as static assets.

| Setting | Value |
| --- | --- |
| Worker name | `wellfed-docs` (must match `name` in `wrangler.jsonc`) |
| Production branch | `master` |
| Build command | `corepack enable && pnpm -C src/web install --frozen-lockfile && pnpm -C src/web generate && pnpm -C src/web --filter @wellfed/docs build` |
| Deploy command | `npx wrangler@4.152.0 deploy --config src/web/apps/docs/wrangler.jsonc` |
| Non-production branch deploy command | `npx wrangler@4.152.0 versions upload --config src/web/apps/docs/wrangler.jsonc` |
| Environment | `NODE_VERSION=22.18.0` |
| Custom domain | `docs.well-fed.dev` |

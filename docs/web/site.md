# well-fed.dev

This page is for maintainers of the project's landing page. The site is one prerendered page in `src/web/apps/site`, served as static files by a Cloudflare Worker.

It is one screen: a headline over the feeder, a short honest line about where Wellfed stands, and links to GitHub and the roadmap. The detail lives in the [README](../../README.md) and the docs, so the page never has to keep a status table in step with them.

## What it is

| Area | Choice |
| --- | --- |
| Build | SvelteKit 3 with `adapter-static`; every route prerendered, `csr = false`, so the page ships no JavaScript |
| Style | The [tokens](../../assets/brand/tokens.json) and `@wellfed/ui`'s base, following the system's light or dark. Typography and hairlines do the work: no product renders, fills, gradients, cards or badges |
| Picture | The feeder's line drawing, centered over a faint dot grid, with four claims pinned to its sides. On a phone the claims move under it as one line of text |
| Brand | The favicon, lockups and theme colors are imported from `assets/brand/`, never copied |
| Requests | None off its own origin: no analytics, CDN or web fonts. `static/_headers` sets a CSP that allows only `'self'` |
| Tests | `svelte-check`, ESLint and Prettier with the workspace, and a Playwright smoke test against `vite preview` at desktop and phone width |

## Keeping it true

The page makes only claims the README already makes: open source, MIT licensed, no account, no vendor cloud, and that Wellfed is in heavy development. The footer keeps the caution to keep another way to feed your pet. If the README's status changes, check the status line in `src/lib/sections/Hero.svelte` still matches it. Outbound links are in `src/lib/links.ts`.

## The drawing

The hero's feeder is line art traced from the 3D model, not a render: `assets/model/lineart.py` writes the visible lines and the part hotspots to `assets/renders/card-lines.json`, and the hero draws them as SVG paths in the theme's line colors. The paths draw themselves in with CSS, and leaders run from parts of the feeder to the claims beside it. Those claims are ones the page already makes: open source, MIT licensed, no account, no vendor cloud. With reduced motion the drawing is simply there.

Because the CSP refuses inline styles, positions are SVG attributes and the animation's timing lives in the component's stylesheet. Retrace after the model changes:

```sh
blender -b assets/model/plaf103.blend -P assets/model/lineart.py
```

## Social card

`static/og.png` is rendered from [assets/brand/og.svg](../../assets/brand/og.svg). Inter must be installed for the text:

```sh
rsvg-convert -w 1200 -h 630 assets/brand/og.svg -o src/web/apps/site/static/og.png
```

## Develop

```sh
pnpm -C src/web install
pnpm -C src/web generate
pnpm -C src/web --filter @wellfed/site dev      # http://localhost:5173
pnpm -C src/web --filter @wellfed/site build    # writes src/web/apps/site/build/
pnpm -C src/web --filter @wellfed/site e2e
```

`make ui` and `make check-web` build, check and test the site with the other apps.

## Deploy

Cloudflare Workers Builds builds from the repository with its Git integration; nothing in the repository holds a Cloudflare credential. The Worker has no script: [`wrangler.jsonc`](../../src/web/apps/site/wrangler.jsonc) serves the prerendered build as static assets, and `static/_headers` sets the response headers. The project settings:

| Setting | Value |
| --- | --- |
| Worker name | `wellfed` (must match `name` in `wrangler.jsonc`) |
| Production branch | `master` |
| Build command | `corepack enable && pnpm -C src/web install --frozen-lockfile && pnpm -C src/web generate && pnpm -C src/web --filter @wellfed/site build` |
| Deploy command | `npx wrangler@4.152.0 deploy --config src/web/apps/site/wrangler.jsonc` |
| Non-production branch deploy command | `npx wrangler@4.152.0 versions upload --config src/web/apps/site/wrangler.jsonc` |
| Environment | `NODE_VERSION=22.18.0` |
| Custom domain | `well-fed.dev` |

Every other branch gets a preview URL from the same settings, which is the place to review a change before it reaches `master`.

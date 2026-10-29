# well-fed.dev

This page is for maintainers of the project's landing page. The site is one prerendered page in `src/web/apps/site`, served as static files by a Cloudflare Worker.

It speaks to two readers: owners who want cloud-free feeding later, and tinkerers who can help now. It is as honest as the [README](../../README.md) about what works.

## What it is

| Area | Choice |
| --- | --- |
| Build | SvelteKit 3 with `adapter-static`; every route prerendered, `csr = false`, so the page ships no JavaScript |
| Style | The [tokens](../../assets/brand/tokens.json) and `@wellfed/ui`'s base, following the system's light or dark. Typography and hairlines do the work: no product renders, fills, gradients, cards or badges |
| Pictures | The hero's line drawing of the feeder and its route diagram (where one feed command travels, stock against Wellfed), drawn in HTML so it reflows on a phone |
| Brand | The favicon, lockups and theme colors are imported from `assets/brand/`, never copied |
| Requests | None off its own origin: no analytics, CDN or web fonts. `static/_headers` sets a CSP that allows only `'self'` |
| Tests | `svelte-check`, ESLint and Prettier with the workspace, and a Playwright smoke test against `vite preview` at desktop and phone width |

## Keeping it true

The claims live in `src/lib/status.ts`:

- the three setup steps, each with its stage (available now, works in testing, in progress, planned);
- the stock and Wellfed comparison. Its stock column comes from the [stock firmware notes](../devices/plaf103/stock/README.md), and each Wellfed entry carries a stage.

Update it by hand when a status changes on the [roadmap](../roadmap.md). Set `UPDATED` to the day. Never claim more than the roadmap and the stock notes do. Outbound links are in `src/lib/links.ts`.

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

# well-fed.dev

The project's landing page: one prerendered page in `src/web/apps/site`, served as static files by Cloudflare Pages. It speaks to two readers, owners who want cloud-free feeding later and tinkerers who can help now, and it is as honest as the [README](../../README.md) about what works.

## What it is

| Area | Choice |
| --- | --- |
| Build | SvelteKit 3 with `adapter-static`; every route prerendered, `csr = false`, so the page ships no JavaScript |
| Style | The [tokens](../../assets/brand/tokens.json) and `@wellfed/ui`'s base, following the system's light or dark. Typography and hairlines do the work: no product renders, gradients, cards or badges |
| Picture | The hero's route diagram (where one feed command travels, stock against Wellfed), drawn in HTML so it reflows on a phone |
| Brand | The favicon, lockups and theme colors are imported from `assets/brand/`, never copied |
| Requests | None off its own origin: no analytics, CDN or web fonts. `static/_headers` sets a CSP that allows only `'self'` |
| Tests | `svelte-check`, ESLint and Prettier with the workspace, and a Playwright smoke test against `vite preview` at desktop and phone width |

## Keeping it true

The claims live in `src/lib/status.ts`:
- the three setup steps, each with its stage (available now, works on the bench, in progress, planned);
- the stock and Wellfed comparison. Its stock column comes from the [stock firmware notes](../devices/plaf103/stock/README.md); each Wellfed entry carries a stage.

Update it by hand when a card's acceptance evidence changes on the [firmware board](../../FIRMWARE_BOARD.md) or the [feature board](../../FEATURE_BOARD.md). Set `UPDATED` to the day, and never claim more than the boards and notes do. Outbound links are in `src/lib/links.ts`.

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

Cloudflare Pages builds from the repository with its Git integration; nothing in the repository holds a Cloudflare credential. The project settings:

| Setting | Value |
| --- | --- |
| Production branch | `master` |
| Build command | `corepack enable && pnpm -C src/web install --frozen-lockfile && pnpm -C src/web generate && pnpm -C src/web --filter @wellfed/site build` |
| Build output directory | `src/web/apps/site/build` |
| Environment | `NODE_VERSION=22.18.0` |
| Custom domain | `well-fed.dev` |

Every other branch gets a preview URL from the same settings, which is the place to review a change before it reaches `master`.

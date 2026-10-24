# Web UI design

The architecture, conventions and quality bar for `petlibre-server`'s web UI: the browser front end that household members use to feed, schedule, follow history and run the server. This is a design (Oct 24, 2026); nothing here is built yet. It serves the [product architecture](../firmware/product-architecture.md) and sits on the [server HTTP API](../api/README.md#server-http-api), which it changes in a few places (listed under [API changes](#api-changes)).

The UI is a single-page app embedded in the server binary. It talks only to the server, never to a feeder, and ships in the same binary as the API it calls, so the two can never disagree about a version.

## Who it is for

| User | Device | What they do |
| --- | --- | --- |
| The owner | A laptop, sometimes a phone | Everything: feeders, schedules, pets and food, history, users, tokens, releases, export |
| Household members | Mostly phones | Feed, check status and meals, edit schedules, read history |

Every screen works at phone width (mobile-first, full parity). Developers use the [bench page](../devices/plaf103/bench.md#progress-page), a separate app that never ships to owners.

## Decisions

| Area | Decision | Rejected, and why |
| --- | --- | --- |
| Framework | Svelte 5 with runes, as the bench | React or Solid: a second component model beside the bench and `@petlibre/ui` |
| App shape | A static SPA embedded in `petlibre-server` with `rust-embed`, served from the same origin as `/api/v1` | SSR (SvelteKit's Node adapter): a Node runtime beside the Rust server breaks the one-binary install. A separate web server: a second thing to deploy and version |
| Meta-framework | SvelteKit with `adapter-static`, `ssr = false` and the hash router | Plain Svelte with a hand-written router, as the bench: about 20 routes with params, guards, layouts and code splitting are ours to maintain. Path routing: SvelteKit's base path is fixed at build time, so the UI would break under a reverse-proxy subpath and the server would need an SPA fallback |
| Server state | TanStack Query (`@tanstack/svelte-query` v6, runes) | Hand-rolled rune stores: caching, deduplication, stale times, retries and invalidation written per resource. SvelteKit `load` functions: no cache across navigations, no background refresh, and mutations and live updates need a second system anyway |
| API types and client | `openapi-typescript` and `openapi-fetch` over the committed `openapi.json` | ts-rs types with a hand-written client (the bench's way): a second derive on every type, and an endpoint list that can drift from the server. Both: two sources that must agree |
| Live updates | One `EventSource` on `/api/v1/stream` per browser, shared by every tab over `BroadcastChannel`, writing into the query cache | One stream per tab: on plain HTTP/1.1 a seventh tab stalls, since browsers allow 6 connections per origin. Web Locks for the leader: unavailable on plain HTTP (see [plain HTTP](#plain-http-on-the-lan)). SharedWorker: missing on Chrome for Android. WebSocket: the API chose SSE |
| Widgets | bits-ui headless primitives for every interactive widget, wrapped and styled in `@petlibre/ui`; plain buttons, links and inputs stay native | Our own widgets: focus traps, roving focus and ARIA for menus and comboboxes are easy to get subtly wrong. shadcn-svelte with Tailwind: a second styling system beside the tokens |
| Styling | CSS custom properties generated from `assets/brand/tokens.json`, and Svelte's scoped styles | Tailwind: long class lists and a second source of design values. CSS-in-JS: runtime cost for nothing Svelte's scoping lacks |
| Design tokens | One file, `assets/brand/tokens.json`, extended with state colors, spacing and a type scale; a build step writes the CSS, and the Flutter theme reads the same file | Hand-copied hex in CSS: drifts from the app and the brand |
| Themes | Light and dark, following the system, with a manual override | Light only: owners check feeders at night, and the dark render (`card-dark`) already exists |
| Charts | LayerChart (Svelte 5, d3 scales, SVG with canvas for dense series), loaded only on chart routes | uPlot: canvas only, an imperative API, weak for bars and accessibility. ECharts: about 300 KB and its own theming. Hand-drawn SVG: axes, zoom and tooltips are ours |
| Text | English in v1, every string through Paraglide JS messages; errors map from typed codes | Inline strings: a later translation touches every file. Several languages now: no translator yet |
| Time | `Intl.DateTimeFormat` with the feeder's IANA zone; no date library | Temporal: not yet in Safari (Oct 2026). date-fns or Day.js: weight for formatting `Intl` already does, and the server (`jiff`) owns schedule arithmetic |
| Units | Grams or ounces and 12- or 24-hour time are household settings on the server | Per-browser locale: two people would see the same plan differently |
| Offline | Show the last data marked with its age, name what is unreachable, disable writes with the reason; no service worker | A PWA cache: a stale shell after the server updates, and the Android app is the installed client. A blocking error page: hides what was last known |
| Browsers | The latest two versions of Chrome, Edge, Firefox and Safari, iOS Safari included | Baseline widely available: holds back popover and other features the latest two versions all have. Polyfills: weight and test matrix |
| First owner | The first visitor to a fresh server creates the owner account | A setup code printed in the log: safer on a shared LAN, but the owner chose the simpler start. CLI only: no way in for owners who never open a terminal |
| CSRF | `SameSite=Strict` cookies; writes need a JSON body and a matching `Origin`, and `Sec-Fetch-Site: same-origin` when the browser sends it | Fetch Metadata alone: browsers send it only to HTTPS and localhost, so on a plain-HTTP LAN every write would be refused. A CSRF token: state in the client the other checks make unnecessary |
| Images | The committed renders in `assets/renders/`, converted to AVIF and WebP with `srcset` by `@sveltejs/enhanced-img`; hotspots from their JSON | Vendor photos: not ours to ship, and the brand says "not affiliated". Hand-exported images: drift from `render.py` |
| Lint and format | ESLint (typescript-eslint `strict-type-checked`, eslint-plugin-svelte, import-boundary rules) and Prettier with the Svelte plugin | Biome: Svelte files only partly checked. oxlint: fewer type-aware and Svelte rules |
| Component tests | Vitest browser mode in real Chromium (`vitest-browser-svelte`); plain Vitest for pure logic | jsdom: no `<dialog>`, popover, real focus or layout, which the widgets depend on |
| End-to-end tests | Playwright against the real `petlibre-server` over a fresh SQLite, with the host-simulated feeder speaking real Noise | A mocked API (MSW): fixtures are a second copy of the API that drifts from the server |

## Workspace

```text
src/web/
  apps/
    bench/                 the bench page (unchanged stack; moves to the new tokens)
    server/                the server UI: SvelteKit, embedded by petlibre-server
  packages/
    tokens/                generated CSS from assets/brand/tokens.json, plus the contrast test
    ui/                    styled components over bits-ui, format helpers, shared by both apps
    api/                   server.gen.ts (from openapi.json) and bench.gen.ts (ts-rs, as today)
```

Allowed imports (each arrow means "may import"):

```text
apps/server ─► packages/ui ─► packages/tokens
     │              └─► bits-ui
     └─► packages/api ─► openapi-fetch
```

- **Only `packages/ui` imports bits-ui.** Apps use our components, so a widget's behavior and look change in one place.
- **Only `packages/api` and the app's `lib/api` touch the network.** No `fetch`, `EventSource` or `BroadcastChannel` anywhere else.
- **The bench keeps its own stack.** It is a dev tool with three views and its ts-rs types; it shares the tokens and components, nothing else.
- ESLint's `no-restricted-imports` enforces these boundaries, as crate boundaries enforce the server's.

## Inside the server app

```text
apps/server/src/
  routes/                  thin: read params, compose features, set titles
    (auth)/login, (auth)/welcome          first-owner setup
    (app)/+layout.svelte                  auth guard, shell, stream, connection banner
    (app)/feeders, feeders/[id], feeders/[id]/plans, feeders/[id]/settings
    (app)/history, (app)/pets, pets/[id], (app)/food
    (app)/settings, settings/users, settings/tokens, settings/releases, settings/data
  lib/
    api/                   client.ts, stream.ts, keys.ts, errors.ts
    features/<area>/       queries.ts (query keys, createQuery, createMutation) and components
    i18n/                  Paraglide's generated messages
```

- **Routes stay thin.** They never call the client; each feature exposes its queries and components through its own `index.ts`, and features don't import each other's internals.
- **`lib/api/client.ts`** builds the `openapi-fetch` client with a base URL resolved against `document.baseURI` (`api/v1/`, no leading slash), so the UI works at `/` and under any proxy subpath.
- **`lib/api/keys.ts`** is the one place query keys are built, so the stream and the features invalidate the same keys.

## Navigation

- **Phones:** a bottom bar with Home, History, Pets and Settings.
- **Wide screens:** the same four as a left rail, with list-and-detail panes where they help (history, plans, users).
- **Settings** holds the household, users and tokens, releases and installs, and export and import; owner-only pages are hidden from members, and the server enforces the same rule.
- **Hash URLs** (`#/feeders/<id>/plans`) survive reloads and can be shared within the household.

## Screens and their data

| Screen | Shows | API |
| --- | --- | --- |
| Welcome | First-owner setup on a fresh server | `GET /setup`, `POST /setup` |
| Login | Password sign-in | `POST /session` |
| Home | Each feeder: render with its food level, connection, power, next meal, Feed now; today's meals | `GET /feeders`, `GET /feeders/{id}`, plans, stream |
| Feeder | Status, panel help with indicator hotspots, recent events, live samples, clock and firmware | `GET /feeders/{id}`, `/events`, `/samples`, stream |
| Plans | The schedule editor: time, portions, repeat days, meal call; conflicts on save | `GET`/`PUT /feeders/{id}/plans` with `If-Match` |
| Feeder settings | Lights, sound, volume, child lock | `GET`/`PATCH /feeders/{id}/settings` with `If-Match` |
| History | Events across feeders, filtered by feeder, kind and date; feeds with requested and delivered portions; the audit log | `GET /feeders/{id}/events`, `GET /audit` |
| Charts | Battery, motor current, Wi-Fi and clock drift over time; grams per day | `GET /feeders/{id}/samples?resolution=raw|hour` |
| Pets | Profiles, weight log, daily target, grams and calories against it | `/pets`, `GET /pets/{id}/intake` |
| Food | Foods, kcal per gram, bag size, food remaining and refill estimate | `/foods`, `GET /feeders/{id}/food` |
| Users and tokens | Household members and roles; phone tokens, shown once | `/users`, `/tokens` |
| Releases | Fetched releases, owner approval, install progress per feeder | `/releases`, `POST /feeders/{id}/install`, stream |
| Data | Export everything; import to move servers | `/export`, `/import` |
| Adopt | Redeem a feeder invite (pasted or scanned link) | `POST /feeders` |
| Household | Name, units, time format | `GET`/`PATCH /household` |

The installer (stock to replacement) is not in v1; the Android app and the CLI install first.

## Server state and live updates

- **Queries.** Every read is a TanStack query keyed from `keys.ts`. Server data is held as immutable snapshots (`$state.raw` where components keep their own). Defaults: 30 s stale time, refetch on window focus, three retries for reads.
- **Mutations never retry.** TanStack's mutation retry stays at 0 everywhere, and a lint rule refuses a `retry` option on a mutation.
- **The stream.** `lib/api/stream.ts` holds the one `EventSource` and maps each typed event to the cache:
  - small resources (feeder status, settings, a command result) carry their new snapshot, written with `setQueryData`;
  - list events (a new feeder event, a sample) prepend to the open history or chart, or invalidate it when it isn't loaded;
  - a `resync` event (the server could not resume from `Last-Event-ID`) invalidates everything.
- **One stream for all tabs.** Tabs elect a leader over `BroadcastChannel` with heartbeats (1 s, takeover after 3 s). The leader holds the `EventSource` and relays each event with its ID; followers apply the same cache updates. If the leader tab closes, another takes over and resumes from the last ID it saw.
- **Connection state** is one store with three states, connected, reconnecting and server unreachable, plus each feeder's own connection from the server. The shell shows a banner naming which is down; data stays visible with its age.

## Feeding safety in the UI

The [API rules](../api/README.md#server-runtime) carry through to the browser:

1. **One intent, one key.** Pressing Feed opens a confirm step with the portion count. Confirming creates the request's `Idempotency-Key` (a UUIDv7 from `crypto.getRandomValues`) and stores it in `sessionStorage` under that feeder until the result is terminal, so a reload mid-feed keeps it.
2. **No automatic retry.** A timeout or a lost connection shows "result unknown" with "Check again" and "Retry with the same request". Retrying resends the same key, which the server and the feeder's request ring and journal turn into the first answer, never a second feed.
3. **Live result.** The command result arrives over the stream: sent, running, done with requested and delivered portions, refused (locked, empty, offline) or uncertain. An uncertain feed reads as uncertain everywhere, never as success. Uncertain is terminal: the feeder may have dispensed, so no retry is offered, as in the app. Only "result unknown" (no answer reached the browser) offers a retry, and only with the same key.
4. **No double press.** The button is disabled from confirm to a terminal result, and the confirm dialog takes focus so a second tap lands on it.

**Concurrent edits.** Plans and settings save with `If-Match`. A `412` opens a conflict view showing what changed on the server since the edit began, and the owner chooses to reload or reapply their change on top. Nothing overwrites silently.

## Errors

- **One error type.** `lib/api/errors.ts` turns every non-2xx response into an `ApiError` carrying the problem's typed `code` from `openapi.json`, and network failures into `ConnectionError`.
- **One message table.** `messageFor(code)` is an exhaustive `switch` over the code enum, so a new server code fails the type check until it has a message.
- **Field errors** come from the problem's field map and attach to their inputs; the server is the validator, and client checks are only for quick feedback (required fields, ranges the feeder's capabilities report).
- **401** sends the user to login and back to the page they were on; **403** shows what needs an owner.

## Look and feel

- **Brand:** Midnight ([brand](../../assets/brand/README.md)): Night `#0B1424`, Navy `#13213A`, Signal `#2563EB` (`signal_on_dark` `#3B82F6` on dark grounds), Mist `#F4F6FA`, and Kibble `#E0A13A` for food states only. Inter: 800 for display, 400 to 600 for UI. Radii 12 (controls), 20 (cards), 14 (icons).
- **A light app with one confident blue,** with dark surfaces for brand moments (login, welcome, empty states), and a full dark theme built from night, navy and the on-dark text colors.
- **Device imagery** comes only from `assets/renders/`: `card` and `card-dark` on cards, `front-food-*` for the food level, `panel` with hotspots for button and indicator help. Indicators are drawn over the render from the hotspot JSON; only the fault light's red is a recorded color, so other lit indicators show as a highlight, not a guessed color. The "not affiliated" line stays wherever the device appears with our brand.
- **Tokens to add to `tokens.json`:** state colors (ok, warning, fault, info) for both themes, a spacing scale, a type scale, focus ring, elevation and motion durations. The generator fails the build when a text and background pair named in the file misses WCAG AA contrast.
- **The bench moves to the same tokens,** keeping its denser layout and monospace numbers.

## Plain HTTP on the LAN

Most owners will open `http://<server>:<port>` with no TLS. That is not a secure context, which rules out several browser APIs:

| Unavailable on plain HTTP | Used instead |
| --- | --- |
| `crypto.randomUUID()` | UUIDv7 from `crypto.getRandomValues` |
| Web Locks (`navigator.locks`) | Leader election over `BroadcastChannel` |
| `Sec-Fetch-Site` and the other Fetch Metadata headers | `Origin` check and a required JSON body |
| `navigator.clipboard` | A selected text field with a copy hint; the clipboard API when available |
| Service workers | Not used |

The UI must work in both contexts, and Playwright runs every test over plain HTTP with one project behind a TLS proxy.

## Serving

- **Embedding.** `petlibre-server --features ui` embeds `apps/server/build/` with `rust-embed`, as the bench does. Release builds enable it; `make check` builds without it, so Rust checks never need Node, and a build without the UI serves a page saying so.
- **Caching.** Hashed files under `_app/immutable/` get `Cache-Control: public, max-age=31536000, immutable`; `index.html` gets `no-cache`.
- **Headers.** A strict CSP (`default-src 'self'`, `script-src 'self'` plus SvelteKit's generated hashes, `style-src 'self'`, `style-src-attr 'unsafe-inline'` for widget positioning, `connect-src 'self'`, `frame-ancestors 'none'`), `X-Content-Type-Options: nosniff` and `Referrer-Policy: no-referrer`.
- **Subpaths.** Behind a proxy at `/petlibre/`, relative asset and API paths keep working; the server redirects a missing trailing slash so relative URLs resolve. Cookies use `Path=/` and `Secure` only when a trusted proxy reports HTTPS.
- **Development.** `pnpm dev` serves the app with hot reload and proxies `api/` to a `petlibre-server` running against the host simulator, so nothing touches a real feeder.

## API changes

What this design asks of the [API doc](../api/README.md), applied there:

- **Types:** the web gets its types and client from `openapi.json` through `openapi-typescript`, and `petlibre-api` drops ts-rs. The bench keeps ts-rs for its own views.
- **CSRF:** the check is `SameSite=Strict`, a JSON body, a matching `Origin`, and `Sec-Fetch-Site: same-origin` when present.
- **Stream events** are typed schemas in `openapi.json` (a `StreamEvent` enum), with a `resync` event when `Last-Event-ID` is too old to resume.
- **New resources:** `/setup` (first owner), `/household` (name, units, time format), `/audit`, `/pets/{id}/intake` and `/feeders/{id}/food` (the derived views).

## Quality standard

The repository's [conventions](../../AGENTS.md) apply where they fit. On top of them:

- **Strict TypeScript:** the existing base (`strict`, `noUncheckedIndexedAccess`, `verbatimModuleSyntax`) everywhere. No `any`, no non-null assertions, and no `as` casts outside the generated boundary. A deliberate exception is a lint disable on one line with its reason, as `#[expect(…, reason)]` is in Rust.
- **Zero warnings:** `svelte-check --fail-on-warnings`, ESLint with no warnings, and `prettier --check`, all in `make check-web`.
- **Runes only.** `$state`, `$derived` and `$props`; no `svelte/store` in app code. Prefer `$derived` to `$effect`; an effect only syncs with something outside Svelte (the DOM, the stream) and says why.
- **Components:** typed props, snippets for content, one concern each. A component that fetches is a feature component; `packages/ui` components never fetch.
- **Generated files are never edited:** `server.gen.ts`, the token CSS and Paraglide's output are regenerated and committed with the change that caused them, and `make check-web` fails on a diff.
- **Accessibility:** WCAG 2.2 AA. Semantic HTML first, every control reachable and operable by keyboard, visible focus, 44 px touch targets on phones, `prefers-reduced-motion` respected, and tests that find elements by role and name.
- **Performance budgets,** checked after the build: at most 150 KB of gzipped JavaScript for the first route, and charts loaded only on chart routes.
- **Dependencies:** exact versions and a frozen lockfile (as today); `minimumReleaseAge` of 7 days in `pnpm-workspace.yaml`; install scripts allowed only for listed packages; and each new package justified in its change, with its license checked.
- **Comments** explain why, as in Rust: an invariant, a browser quirk, a feeding-safety rule.

## Tests by layer

| Layer | Tool | Covers |
| --- | --- | --- |
| Tokens | Vitest | Generation is stable, and every named pair meets AA contrast in both themes |
| Logic | Vitest | Formatting with household units and zones, the feed state machine, error mapping, leader election against a fake channel, stream-to-cache mapping |
| Components | Vitest browser mode | `packages/ui` and feature components in real Chromium, both themes, keyboard paths |
| Contract | `make check-web` | `server.gen.ts` regenerated from `openapi.json` with no diff |
| End to end | Playwright | The real server and simulator, on desktop and phone projects: every screen, axe-core on every route in both themes, offline banners, the conflict view |
| Feeding safety | Playwright | A double press, a connection cut mid-feed and a retry with the same key each end with exactly one feed in the simulator; an uncertain feed shows as uncertain |

Visual screenshot tests are left out of v1: fonts and rendering differ across machines, and role-based assertions catch what matters.

## Build order

1. **Foundations:** the token pipeline and contrast test, `@petlibre/ui` on bits-ui, the bench moved to the new tokens, ESLint and Prettier in `make check-web`.
2. **Skeleton:** the SvelteKit app, client, errors, stream with leader election, auth and first-owner setup, Paraglide, the shell and navigation, and the Playwright harness against the server and simulator.
3. **Feed, status and schedules:** home, feeder, plans and settings, with the feeding-safety tests.
4. **History and charts.**
5. **Pets, food and the derived views.**
6. **Admin:** users and tokens, releases and installs, export and import, adoption.

Each step waits on the server endpoints it uses; until `petlibre-server` exists, step 1 can proceed alone.

## Open questions

- Does SvelteKit's generated CSP cover the hash-router fallback page's inline bootstrap script, or does it need a hash added by the build?
- Which bits-ui widgets need `style-src-attr 'unsafe-inline'`, and can positioning move to CSS anchor positioning on the target browsers?
- The first-visitor rule leaves a window where anyone on the LAN can claim a fresh server (`/setup` closes once the first account exists). Should the server also log the claim prominently, so an owner can tell if someone beat them to it?
- Image sizes for the renders in `srcset`, and whether 1024 px sources are enough for wide screens.
- Whether a component gallery page (for design review of `@petlibre/ui`) is worth its upkeep.

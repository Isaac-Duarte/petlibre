# TypeScript standard

These are the rules for the web frontends in `src/web/` (TypeScript and Svelte), why each exists, and what enforces it. [AGENTS.md](../../AGENTS.md#web-conventions) keeps the short form. The [web UI design](../web/README.md#quality-standard) adds what is specific to the UI: accessibility, performance budgets and tests by layer.

Every rule marked with a tool runs in `make check-web`.

## Layout

```text
src/web/
  apps/bench/          the bench page (Svelte 5, which wellfed-bench serve --ui embeds with --features ui)
  apps/site/           the project's landing page, prerendered to static files (see docs/web/site.md)
  packages/tokens/     CSS generated from assets/brand/tokens.json, and the contrast check
  packages/ui/         the page base and components over the tokens and bits-ui
  packages/api/        typed clients; types generated from Rust
  scripts/             workspace checks run by Node (the license check)
  eslint.config.js     the lint rules below
```

## Rules

| Rule | Why | Enforced by |
| --- | --- | --- |
| **Strict types.** `strict`, `noUncheckedIndexedAccess`, `noImplicitOverride` and `verbatimModuleSyntax` everywhere. | Index reads and optional fields are handled where they happen. | `tsconfig.base.json`; `tsc` and `svelte-check --fail-on-warnings` |
| **No `any`, no `!`, no `as`.** Narrow with a check (`instanceof`, `in`, a type guard) instead of asserting. `as const` is fine. | An assertion moves a wrong assumption from where it is made to where it crashes. | ESLint (`no-explicit-any`, `no-non-null-assertion`, `consistent-type-assertions: never`) |
| **The API layer is the boundary.** `packages/api` (and later an app's `lib/api`) is where a response body becomes a generated type. Assertions are allowed there and nowhere else. | One place to audit what the server is trusted to send. | ESLint override for `packages/api/**` |
| **Generated contracts.** Rust types arrive through ts-rs (`*.gen.ts`) and later `openapi.json`. Colors, spacing and type arrive through `@wellfed/tokens`. Never hand-write a copy of a Rust type or a hex value. Generated files are rewritten, never edited, and committed with the change that caused them. | Two copies drift. | `tokens.test.ts` fails when `tokens.gen.css` is stale; ESLint and Prettier skip `*.gen.*` |
| **Only `packages/ui` imports bits-ui.** Apps use its wrappers. | A widget's behavior and look change in one place. | ESLint (`no-restricted-imports`) |
| **Only the API layer touches the network.** No `fetch`, `EventSource`, `BroadcastChannel`, `WebSocket` or `XMLHttpRequest` elsewhere. | Retry and idempotency rules live in one place. | ESLint (`no-restricted-globals`) |
| **Runes only.** `$state`, `$derived` and `$props`; no `svelte/store`. Prefer `$derived` to `$effect`. An effect only syncs with something outside Svelte, and says why. | One reactivity model. | ESLint for stores; review for effects |
| **Feeding safety.** Mutations never retry. A feed keeps one idempotency key from confirm to a terminal result, and an uncertain feed is never retried. | A retried feed can dispense twice. | Review now; a lint rule when the server UI adds mutations |
| **Zero warnings.** typescript-eslint `strict-type-checked` and `stylistic-type-checked`, eslint-plugin-svelte, `--max-warnings 0`. | A warning nobody fixes hides the next one. | `pnpm lint` |
| **Deliberate exceptions.** A disable covers one line and gives its reason after `--`, as `#[expect(…, reason)]` does in Rust. | The next reader knows whether it still applies. | ESLint (`eslint-comments/require-description`, `no-unused-disable`) |
| **One format.** Prettier with the Svelte plugin, 120 columns. | No formatting in review. | `pnpm format:check` |
| **Comments explain why:** an invariant, a browser quirk, a feeding-safety rule. | As in Rust. | Review |

## Dependencies

Set in [pnpm-workspace.yaml](../../src/web/pnpm-workspace.yaml) and checked on every install and in `make check-web`:

- **Exact versions and a frozen lockfile.** `savePrefix: ""` pins what `pnpm add` writes; every check installs with `--frozen-lockfile`.
- **A week before install.** `minimumReleaseAge` is 7 days, so a compromised release is usually pulled before it reaches the lockfile. pnpm re-verifies the lockfile against it on install. An exception names the exact version and when it can be dropped.
- **No install scripts** unless a package is listed under `allowBuilds` (none today), with `strictDepBuilds` failing the install otherwise.
- **Licenses.** `pnpm run licenses` ([scripts/licenses.ts](../../src/web/scripts/licenses.ts)) checks every installed package, dev tools included, against the same permissive set as [deny.toml](../../deny.toml), plus 0BSD, BlueOak, CC-BY-4.0 (data) and OFL-1.1 (the bundled fonts).
- **Justify each package** in the change that adds it: what it does that the platform or an existing package doesn't.

## Commands

From `src/web/`, with Node 22.18 or later (it runs the TypeScript scripts directly) and pnpm 11:

| Command | Does |
| --- | --- |
| `pnpm install --frozen-lockfile` | Install exactly the lockfile |
| `pnpm generate` | Rewrite `tokens.gen.css` from `assets/brand/tokens.json`; fails if a named pair misses its contrast minimum |
| `pnpm check` | ESLint, Prettier, licenses, `tsc` for the scripts, then each package's `svelte-check` or `tsc` |
| `pnpm test` | Vitest in each package (the contrast and drift tests included) |
| `pnpm format` | Rewrite formatting |
| `make check-web` (repository root) | All of the above, the bench build, and Playwright against the real `wellfed-bench serve --ui` on a fake relay board (built with `--features ui,wellfed-bench/fake-board`) |

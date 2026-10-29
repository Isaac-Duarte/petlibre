import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { defineConfig, devices } from "@playwright/test";

// End to end against the real binary: `e2e/serve.mjs` runs
// `wellfed-bench serve --ui` with a scripted relay board and a stand-in
// flash client, three times over:
// - fixtures: three finished integration runs, a capture, sessions, notes
//   and a bank ledger, with no job, so the evidence views read them;
// - writing: a UART write that lasts the whole run;
// - idle: nothing, for the tests that run jobs of their own.
// Nothing touches hardware.
// The site and the docs take 8798 and 8799, and run alongside.
const PORT = Number(process.env.E2E_PORT ?? 8797);
const WRITING = Number(process.env.E2E_WRITING_PORT ?? 8795);
const IDLE = Number(process.env.E2E_IDLE_PORT ?? 8796);

// The tests run clients against the same binary and the idle checkout;
// the config is read again in every worker, so these reach the tests.
process.env.BENCH_BIN ??= resolve(import.meta.dirname, "../../../../target/debug/wellfed-bench");
process.env.E2E_WRITING_URL = `http://127.0.0.1:${String(WRITING)}`;
process.env.E2E_IDLE_URL = `http://127.0.0.1:${String(IDLE)}`;

// Each server's checkout is target/e2e/roots/PORT, which serve.mjs clears
// first, so a run killed outright leaves one behind that the next removes.
// A Unix socket's path holds at most 107 bytes: a checkout deep enough to
// pass that (a Git worktree, say) gets a fresh temporary directory instead.
// Set once, in the main process, so every worker sees the same roots.
const ROOTS = resolve(import.meta.dirname, "../../../../target/e2e/roots");
const SOCKET = "private/bench.sock";
function root(port: number): string {
  const name = `E2E_ROOT_${String(port)}`;
  const fixed = join(ROOTS, String(port));
  const chosen =
    process.env[name] ??
    (Buffer.byteLength(join(fixed, SOCKET)) <= 107
      ? fixed
      : mkdtempSync(join(tmpdir(), `wellfed-e2e-${String(port)}-`)));
  process.env[name] = chosen;
  return chosen;
}
process.env.E2E_IDLE_ROOT = root(IDLE);

// SIGINT, so each server cancels its job, waits for the cleanup and
// removes its checkout; Playwright's default is SIGKILL, which leaves
// them all behind.
const serve = (port: number, mode: string) => ({
  command: ["node", "e2e/serve.mjs", String(port), mode, root(port)].join(" "),
  url: `http://127.0.0.1:${String(port)}/api/state`,
  reuseExistingServer: false,
  timeout: 20_000,
  gracefulShutdown: { signal: "SIGINT" as const, timeout: 15_000 },
});

export default defineConfig({
  testDir: "e2e",
  fullyParallel: true,
  reporter: process.env.CI ? "line" : "list",
  use: { baseURL: `http://127.0.0.1:${String(PORT)}` },
  projects: [
    { name: "desktop", use: { ...devices["Desktop Chrome"] } },
    { name: "phone", use: { ...devices["Pixel 7"] } },
  ],
  webServer: [serve(PORT, "fixtures"), serve(WRITING, "writing"), serve(IDLE, "idle")],
});

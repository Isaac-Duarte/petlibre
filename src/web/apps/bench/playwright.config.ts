import { defineConfig, devices } from "@playwright/test";

// End to end against the real binary: `e2e/serve.mjs` writes a synthetic
// private/ tree (a live integration run, three finished ones, a capture)
// and serves it with `wellfed-bench --ui`. Nothing touches hardware.
const PORT = Number(process.env.E2E_PORT ?? 8797);

export default defineConfig({
  testDir: "e2e",
  fullyParallel: true,
  reporter: process.env.CI ? "line" : "list",
  use: { baseURL: `http://127.0.0.1:${PORT}` },
  projects: [
    { name: "desktop", use: { ...devices["Desktop Chrome"] } },
    { name: "phone", use: { ...devices["Pixel 7"] } },
  ],
  webServer: {
    command: `node e2e/serve.mjs ${PORT}`,
    url: `http://127.0.0.1:${PORT}/api/status`,
    reuseExistingServer: false,
    timeout: 20_000,
  },
});

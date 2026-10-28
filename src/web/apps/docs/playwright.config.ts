import { defineConfig, devices } from "@playwright/test";

// The built site, served by e2e/serve.ts as the Cloudflare Worker will
// serve build/: static files and the Pagefind index, no server code.
// `make check-web` builds first.
const PORT = Number(process.env.E2E_PORT ?? 8799);

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
    command: `node e2e/serve.ts`,
    env: { PORT: String(PORT) },
    url: `http://127.0.0.1:${PORT}/`,
    reuseExistingServer: false,
    timeout: 20_000,
  },
});

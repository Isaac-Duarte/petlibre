import { expect, test } from "@playwright/test";

// The built site at desktop and phone width, served as the Worker serves it.
test("renders a doc from its own origin only", async ({ page, baseURL }) => {
  const origin = new URL(baseURL ?? "").origin;
  const offsite: string[] = [];
  const errors: string[] = [];
  page.on("request", (request) => {
    if (new URL(request.url()).origin !== origin) offsite.push(request.url());
  });
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });

  await page.goto("/firmware/motor-pulse");

  await expect(page.locator("h1")).toHaveCount(1);
  await expect(page.locator(".alert-warning").first()).toBeVisible();
  await expect(page.getByRole("link", { name: "Edit this page on GitHub" })).toHaveAttribute(
    "href",
    "https://github.com/Isaac-Duarte/wellfed/edit/master/docs/firmware/motor-pulse.md",
  );

  await page.waitForLoadState("networkidle");
  expect(offsite).toEqual([]);
  expect(errors).toEqual([]);
});

test("every sidebar link opens a page", async ({ request, page }) => {
  await page.goto("/");
  const hrefs = await page.locator("#sidebar a").evaluateAll((links) => links.map((a) => a.getAttribute("href")));

  expect(hrefs.length).toBeGreaterThan(50);
  for (const href of hrefs) {
    expect((await request.get(href ?? "")).status(), href ?? "").toBe(200);
  }
});

test("search finds a page and opens it", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Search docs" }).click();
  await page.getByRole("combobox").fill("noise handshake");

  const first = page.getByRole("option").first();
  await expect(first).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(page).not.toHaveURL(/\/$/);
  await expect(page.locator("h1")).toBeVisible();
});

test("a page opens on its overview and remembers Details", async ({ page }) => {
  const errors: string[] = [];
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });

  await page.goto("/devices/plaf103");
  const details = page.getByRole("heading", { name: "Sold as" });
  await expect(page.locator(".diagram svg").first()).toBeVisible();
  await expect(details).toBeHidden();

  await page.getByRole("button", { name: "Show the details" }).click();
  await expect(details).toBeVisible();
  await expect(page.getByRole("button", { name: "Details", exact: true })).toHaveAttribute("aria-pressed", "true");

  await page.goto("/firmware/bank-policy");
  await expect(page.getByRole("heading", { name: "Capacities" })).toBeVisible();
  await page.getByRole("button", { name: "Overview", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Capacities" })).toBeHidden();
  expect(errors).toEqual([]);
});

test("a link into the details opens them", async ({ page }) => {
  await page.goto("/devices/plaf103/flash-layout#flash-clock");

  await expect(page.getByRole("heading", { name: "Flash clock" })).toBeVisible();
});

test("without JavaScript a page shows in full", async ({ browser }) => {
  const context = await browser.newContext({ javaScriptEnabled: false });
  const page = await context.newPage();

  await page.goto("/devices/plaf103");

  await expect(page.getByRole("heading", { name: "Sold as" })).toBeVisible();
  await expect(page.locator(".mode-switch")).toBeHidden();
  await context.close();
});

test("never scrolls sideways", async ({ page }) => {
  for (const path of ["/", "/development/rust", "/firmware/product-architecture", "/devices/plaf103"]) {
    await page.goto(path);
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
    expect(overflow, path).toBeLessThanOrEqual(0);
  }
});

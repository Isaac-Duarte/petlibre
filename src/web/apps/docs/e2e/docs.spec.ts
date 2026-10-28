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

test("never scrolls sideways", async ({ page }) => {
  for (const path of ["/", "/development/rust", "/firmware/product-architecture"]) {
    await page.goto(path);
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
    expect(overflow, path).toBeLessThanOrEqual(0);
  }
});

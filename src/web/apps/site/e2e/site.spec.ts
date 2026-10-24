import { expect, test } from "@playwright/test";

// A smoke test of the built page at desktop and phone width: it renders,
// stays on its own origin, and keeps the claims the page makes in sight.
test("renders the landing page from its own origin only", async ({ page, baseURL }) => {
  const origin = new URL(baseURL ?? "").origin;
  const offsite: string[] = [];
  const errors: string[] = [];
  page.on("request", (request) => {
    if (new URL(request.url()).origin !== origin) offsite.push(request.url());
  });
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });

  await page.goto("/");

  await expect(page.locator("h1")).toHaveCount(1);
  await expect(page.locator("h1")).toHaveText("Open firmware for your pet feeder.");
  await expect(page.getByRole("link", { name: "View on GitHub" })).toHaveAttribute(
    "href",
    "https://github.com/Isaac-Duarte/wellfed",
  );
  await expect(page.getByText("It is experimental and does not feed a pet yet.")).toBeVisible();
  await expect(page.getByText("Not affiliated with or endorsed by PETLIBRO", { exact: false })).toBeVisible();

  // Every step and every Wellfed entry carries its stage in words.
  const steps = page.locator("#how ol > li");
  await expect(steps).toHaveCount(3);
  for (const stage of ["Works on the bench", "In progress", "Planned"]) {
    await expect(steps.getByText(stage)).toBeVisible();
  }
  const rows = page.locator("#compare tbody tr");
  await expect(rows).toHaveCount(7);
  for (const row of await rows.all()) {
    await expect(row.locator(".ours > span").last()).toHaveText(
      /^(Available now|Works on the bench|In progress|Planned)$/,
    );
  }

  await page.waitForLoadState("networkidle");
  expect(offsite).toEqual([]);
  expect(errors).toEqual([]);
});

test("loads every image and ships no script", async ({ page }) => {
  await page.goto("/");

  for (const image of await page.locator("img").all()) {
    await expect.poll(() => image.evaluate((img: HTMLImageElement) => img.complete && img.naturalWidth > 0)).toBe(true);
  }
  await expect(page.locator("script")).toHaveCount(0);
});

test("never scrolls sideways", async ({ page }) => {
  await page.goto("/");

  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
  expect(overflow).toBeLessThanOrEqual(0);
});

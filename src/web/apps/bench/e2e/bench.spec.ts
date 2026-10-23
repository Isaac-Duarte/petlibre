import { expect, test } from "@playwright/test";

test("the overview shows the live run, the feeder and the bench", async ({ page }) => {
  await page.goto("/");

  await expect(page.getByRole("heading", { name: "itest", level: 1 })).toBeVisible();
  await expect(page.getByText("Integration run in progress")).toBeVisible();
  await expect(page.getByText("No local server configured")).toBeVisible();
  await expect(page.getByLabel("Bench hardware and tools")).toContainText("Power");
});

test("the tests view follows the live run and charts every recorded one", async ({ page }) => {
  await page.goto("/#/tests");

  const live = page.getByRole("region", { name: "Run in progress" });
  await expect(live).toContainText("itest-e2e-04");
  await expect(live).toContainText("Waiting for fresh health: health is 80 s old");

  const history = page.getByRole("region", { name: "Run history" });
  await expect(history).toContainText("3 recorded runs");
  await expect(history.getByRole("img", { name: /itest-e2e-02: requests fail in 21s/ })).toBeVisible();
  // Failed twice after one pass: a single change, so a regression rather than flaky.
  const requests = history.getByRole("row").filter({ has: page.getByRole("rowheader", { name: /^requests/ }) });
  await expect(requests).toContainText("33%");
  await expect(requests).not.toContainText("flaky");
});

test("a run opens with its verdicts, silence and evidence", async ({ page }) => {
  await page.goto("/#/tests/itest-e2e-03");

  const drawer = page.getByRole("dialog", { name: "Run itest-e2e-03" });
  await expect(drawer).toContainText("The feeder went silent");
  await expect(drawer).toContainText("petlibre-bench flash-prepare itest-e2e-03-recover");
  await drawer.getByText("private/captures/itest-e2e-03-uart-01").click();
  await expect(drawer).toContainText("PLFW1 start build=4d37c21a bank=1");

  await page.keyboard.press("Escape");
  await expect(drawer).toBeHidden();
  await expect(page).toHaveURL(/#\/tests$/);
});

test("the run builder writes the command and warns about skips", async ({ page }) => {
  await page.goto("/#/tests");
  const builder = page.getByRole("region", { name: "Start a run" });

  await builder.getByRole("textbox", { name: "Name" }).fill("itest-e2e-05");
  await expect(builder.locator("pre")).toHaveText("petlibre-bench --ui itest itest-e2e-05");
  await expect(builder).toContainText("will be skipped");

  await builder.getByRole("radio", { name: "Smoke" }).click();
  await builder.getByLabel(/--power/).check();
  await expect(builder.locator("pre")).toHaveText("petlibre-bench --ui itest itest-e2e-05 --tier smoke --power");
});

test("the log shows the run's notes, filterable", async ({ page }) => {
  await page.goto("/#/log");

  const log = page.getByRole("log", { name: "Live log" });
  await expect(log).toContainText("- requests: ping, health");
  await page.getByRole("searchbox", { name: "Filter log lines" }).fill("waiting");
  await expect(log).toContainText("Waiting for fresh health");
  await expect(log).not.toContainText("ota-update: pass");
});

test("both themes render without horizontal page scroll", async ({ page }) => {
  for (const colorScheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme });
    for (const view of ["overview", "tests", "sessions", "log"]) {
      await page.goto(`/#/${view}`);
      await page.waitForLoadState("networkidle").catch(() => {});
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
      expect(overflow, `${view} in ${colorScheme}`).toBeLessThanOrEqual(0);
    }
  }
});

import { expect, test, type Locator } from "@playwright/test";

test("the overview shows the live run, its log, the feeder and the bench", async ({ page }) => {
  await page.goto("/");

  await expect(page.getByRole("heading", { name: "itest", level: 1 })).toBeVisible();
  await expect(page.getByRole("log", { name: "Live log" })).toBeVisible();
  await expect(page.getByText("No local server configured")).toBeVisible();
  await expect(page.getByLabel("Bench hardware and tools")).toContainText("Power");
});

test("the overview coaches a guided QA run from the latest capture", async ({ page }) => {
  await page.goto("/");

  const qa = page.getByLabel("Guided QA");
  await expect(qa).toContainText("Look at the Wi-Fi light");
  await expect(qa).toContainText("Lit: press Dispense. Dark: press Lock.");
  await expect(qa).toContainText("Indicators");
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
  await expect(drawer).toContainText("wellfed-bench flash-prepare itest-e2e-03-recover");
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
  await expect(builder.locator("pre")).toHaveText("wellfed-bench --ui itest itest-e2e-05");
  await expect(builder).toContainText("will be skipped");

  await builder.getByRole("radio", { name: "Smoke" }).click();
  await builder.getByLabel(/--power/).check();
  await expect(builder.locator("pre")).toHaveText("wellfed-bench --ui itest itest-e2e-05 --tier smoke --power");
});

test("the overview log shows the run's notes, filterable", async ({ page }) => {
  await page.goto("/");

  const log = page.getByRole("log", { name: "Live log" });
  await expect(log).toContainText("- requests: ping, health");
  await page.getByRole("searchbox", { name: "Filter log lines" }).fill("waiting");
  await expect(log).toContainText("Waiting for fresh health");
  await expect(log).not.toContainText("ota-update: pass");
});

test("both themes render without horizontal page scroll", async ({ page }) => {
  for (const colorScheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme });
    for (const view of ["overview", "tests", "sessions", "tests/itest-e2e-03", "sessions/flash-e2e-01"]) {
      await page.goto(`/#/${view}`);
      await page.waitForLoadState("networkidle").catch(() => {
        // The event stream never goes idle; the overflow check below is what matters.
      });
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
      expect(overflow, `${view} in ${colorScheme}`).toBeLessThanOrEqual(0);
    }
  }
});

test.describe("at phone width", () => {
  test.skip(({ isMobile }) => !isMobile, "phone layout only");

  /** Every matched element is at least a 44 px square, the touch target minimum. */
  async function expectThumbSized(targets: Locator) {
    const count = await targets.count();
    expect(count).toBeGreaterThan(0);
    for (let i = 0; i < count; i++) {
      const box = await targets.nth(i).boundingBox();
      expect(box, `target ${String(i)}`).not.toBeNull();
      expect(box?.height ?? 0, `target ${String(i)} height`).toBeGreaterThanOrEqual(44);
      expect(box?.width ?? 0, `target ${String(i)} width`).toBeGreaterThanOrEqual(44);
    }
  }

  test("navigation and the log's controls are thumb-sized", async ({ page }) => {
    await page.goto("/");

    await expectThumbSized(page.getByRole("navigation", { name: "Views" }).getByRole("link"));
    await expectThumbSized(page.getByRole("button", { name: /^Theme/ }));
    const log = page.getByRole("region", { name: "Live log" });
    await expectThumbSized(log.getByRole("searchbox", { name: "Filter log lines" }));
    await expectThumbSized(log.locator("label.follow"));
    await expectThumbSized(log.getByRole("button", { name: "Copy" }));
  });

  test("long log lines scroll inside the log, not the page", async ({ page }) => {
    await page.goto("/");

    const log = page.getByRole("log", { name: "Live log" });
    await expect(log).toContainText("- requests: ping, health");
    const box = await log.boundingBox();
    expect((box?.x ?? 0) + (box?.width ?? 0)).toBeLessThanOrEqual(page.viewportSize()?.width ?? 0);
    expect(await log.evaluate((el) => el.scrollWidth > el.clientWidth)).toBe(true);
  });

  test("the run builder stacks each scenario with its tier and flags", async ({ page }) => {
    await page.goto("/#/tests");
    const builder = page.getByRole("region", { name: "Start a run" });

    await expectThumbSized(builder.getByRole("radio"));
    await expectThumbSized(builder.locator("label.hit"));
    // The tier column is shown, not dropped, at phone width.
    await expect(builder.getByText("standard", { exact: true }).first()).toBeVisible();
    await builder.getByRole("checkbox", { name: "Only requests" }).click();
    await expect(builder.locator("pre")).toContainText("requests");
  });

  test("run history, the run drawer and a session's Close are thumb-sized", async ({ page }) => {
    await page.goto("/#/tests");
    const history = page.getByRole("region", { name: "Run history" });
    await expectThumbSized(history.locator("th.run a").first());
    await expectThumbSized(history.locator("td.cell a").first());

    await page.goto("/#/tests/itest-e2e-03");
    await expectThumbSized(
      page.getByRole("dialog", { name: "Run itest-e2e-03" }).getByRole("button", { name: "Close" }),
    );

    await page.goto("/#/sessions/flash-e2e-01");
    await expectThumbSized(page.getByRole("region", { name: "flash-e2e-01" }).getByRole("link", { name: "Close" }));
  });

  test("sessions keep their evidence parts and recent commands their start time", async ({ page }) => {
    await page.goto("/#/sessions");

    const sessions = page.getByRole("region", { name: "Sessions" });
    await expectThumbSized(sessions.getByRole("link", { name: "flash-e2e-01" }));
    await expect(sessions.getByText("inspect", { exact: true })).toBeVisible();

    await page.goto("/");
    // The start time stacks under the command rather than being dropped.
    await expect(page.getByRole("region", { name: "Recent commands" }).getByText(/^\d{1,2}:\d{2}/)).toBeVisible();
  });
});

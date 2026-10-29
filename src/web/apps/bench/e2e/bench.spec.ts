import { spawn, type ChildProcess } from "node:child_process";
import { expect, test, type Locator } from "@playwright/test";

// The fixtures server is the default base URL; see playwright.config.ts.
const WRITING = process.env.E2E_WRITING_URL ?? "";
const IDLE = process.env.E2E_IDLE_URL ?? "";
const IDLE_ROOT = process.env.E2E_IDLE_ROOT ?? "";
const BENCH_BIN = process.env.BENCH_BIN ?? "";

test("the overview shows the server's job, its log, the stock server and the bench", async ({ page }) => {
  await page.goto(`${WRITING}/`);

  const job = page.getByRole("region", { name: "Current job" });
  await expect(job.getByRole("heading", { name: "flash-execute", level: 1 })).toBeVisible();
  await expect(job).toContainText("flash-e2e-05");
  await expect(job).toContainText("3 of 19 sectors");
  await expect(page.getByRole("log", { name: "Live log" })).toBeVisible();
  // The stock server is not asked during the write.
  await expect(page.getByRole("region", { name: "Stock server" })).toContainText("Asking the local server");

  // The board and the UART adapter are the server's to report.
  const strip = page.getByLabel("Bench hardware and tools");
  await expect(strip).toContainText(/Relay board\s*held by the server/);
  await expect(strip).toContainText(/UART\s*missing/);
  await expect(strip).toContainText(/Power\s*on/);
});

test("a UART write puts its banner, with the sectors verified, on every view", async ({ page }) => {
  for (const view of ["overview", "tests", "sessions"]) {
    await page.goto(`${WRITING}/#/${view}`);
    const banner = page.getByRole("alert", { name: "UART write in progress" });
    await expect(banner).toContainText("don't touch the bench");
    await expect(banner).toContainText("3 of 19 sectors verified");
  }
  await expect(page).toHaveTitle(/^WRITING/);
});

test("a page opened during a write asks for no view that reads files", async ({ page }) => {
  const asked: string[] = [];
  page.on("request", (request) => {
    const path = new URL(request.url()).pathname;
    if (/^\/api\/(health|sessions|tests|evidence|feeder)/.test(path)) asked.push(path);
  });

  await page.goto(`${WRITING}/`);
  await expect(page.getByRole("alert", { name: "UART write in progress" })).toBeVisible();
  // Past the first round of every poll.
  await page.waitForTimeout(6_000);

  // What it has not read shows as deferred, never as missing.
  await expect(page.getByRole("region", { name: "Latest capture" })).toContainText("Deferred until the write ends");
  await expect(page.getByRole("region", { name: "Recent commands" })).toContainText("Deferred until the write ends");
  await page.goto(`${WRITING}/#/tests/itest-e2e-03`);
  await expect(page.getByRole("dialog", { name: "Run itest-e2e-03" })).toContainText("Deferred until the write ends");
  await page.goto(`${WRITING}/#/sessions/flash-e2e-01`);
  await expect(page.getByRole("region", { name: "Sessions" })).toContainText("Deferred until the write ends");
  await expect(page.getByRole("region", { name: "flash-e2e-01" })).toContainText("Deferred until the write ends");
  expect(asked).toEqual([]);
});

test("activity and relays read the live state", async ({ page }) => {
  await page.goto(`${WRITING}/`);

  const activity = page.getByRole("region", { name: "Activity" });
  await expect(activity).toContainText("update execute");
  await expect(activity).toContainText("flash-execute flash-e2e-05");

  const relays = page.getByRole("region", { name: "Relays" });
  await expect(relays.getByRole("listitem").first()).toContainText("Feeder power");
  await expect(relays.getByRole("listitem").first()).toContainText("on");
  await expect(relays).toContainText("holding the bench since");
});

test("the operator's notes and the feeder's banks read the live state", async ({ page }) => {
  await page.goto("/");

  const operator = page.getByRole("region", { name: "Operator" });
  await expect(operator).toContainText("Install the candidate over UART");
  await expect(operator.getByRole("list", { name: "Operator's next steps" })).toContainText("Run the smoke tier");
  const timers = operator.getByRole("list", { name: "Timers" });
  await expect(timers).toContainText("expects PLFW1 start");
  // 900 s from a minute before the server started, and 600 s from an hour before.
  await expect(timers.getByRole("listitem").filter({ hasText: "boot" })).toContainText(/1[34]m \d\ds/);
  await expect(timers.getByRole("listitem").filter({ hasText: "soak" })).toContainText(/out (49|50)m \d\ds ago/);
  await expect(operator.getByRole("region", { name: "Evidence for QA9" })).toContainText("1 pass, 1 fail");

  const feeder = page.getByRole("region", { name: "Feeder", exact: true });
  const banks = feeder.getByRole("list", { name: "Banks" });
  await expect(banks).toContainText("build 4d37c21a00ff");
  await expect(banks).toContainText("confirmed by the start banner");
  await expect(banks).toContainText("disputed: the device API's status reported build 77aa00bb11cc");
  await expect(feeder).toContainText("Journal high water");
  await expect(feeder).toContainText("not known yet");
});

test("an idle bench shows no banner and no job", async ({ page }) => {
  await page.goto("/");

  await expect(page.getByRole("region", { name: "Relays" })).toContainText("holding the bench since");
  await expect(page.getByRole("alert", { name: "UART write in progress" })).toHaveCount(0);
  await expect(page.getByText("No job yet")).toBeVisible();
  await expect(page).toHaveTitle("wellfed bench");
  await expect(page.getByText("No local server configured")).toBeVisible();
});

test("the overview coaches a guided QA run from the latest capture", async ({ page }) => {
  await page.goto("/");

  const qa = page.getByLabel("Guided QA");
  await expect(qa).toContainText("Look at the Wi-Fi light");
  await expect(qa).toContainText("Lit: press Dispense. Dark: press Lock.");
  await expect(qa).toContainText("Indicators");
});

test("the tests view charts every recorded run", async ({ page }) => {
  await page.goto("/#/tests");

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
  await expect(builder.locator("pre")).toHaveText("wellfed-bench itest itest-e2e-05");
  await expect(builder).toContainText("will be skipped");

  await builder.getByRole("radio", { name: "Smoke" }).click();
  await builder.getByLabel(/--power/).check();
  await expect(builder.locator("pre")).toHaveText("wellfed-bench itest itest-e2e-05 --tier smoke --power");
});

test("the overview log shows the job's output from the server, filterable", async ({ page }) => {
  await page.goto(`${WRITING}/`);

  const log = page.getByRole("log", { name: "Live log" });
  await expect(log).toContainText("Verified changed sectors: 3/19");
  await page.getByRole("searchbox", { name: "Filter log lines" }).fill("loader");
  await expect(log).toContainText("Loader answered");
  await expect(log).not.toContainText("Verified changed sectors");
});

test("both themes render without horizontal page scroll", async ({ page }) => {
  const views = [
    "/#/overview",
    "/#/tests",
    "/#/sessions",
    "/#/tests/itest-e2e-03",
    "/#/sessions/flash-e2e-01",
    `${WRITING}/#/overview`,
  ];
  for (const colorScheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme });
    for (const view of views) {
      await page.goto(view);
      await page.waitForLoadState("networkidle").catch(() => {
        // The event stream never goes idle; the overflow check below is what matters.
      });
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
      expect(overflow, `${view} in ${colorScheme}`).toBeLessThanOrEqual(0);
    }
  }
});

/** A client command on the idle bench. */
function client(words: string[]): ChildProcess {
  return spawn(BENCH_BIN, ["--root", IDLE_ROOT, ...words], { stdio: "ignore" });
}

function ended(child: ChildProcess): Promise<number> {
  return new Promise((done) => {
    child.on("exit", (code) => {
      done(code ?? 1);
    });
  });
}

test.describe("a write's life on the idle bench", () => {
  // These run jobs on one shared server: once, in order.
  test.describe.configure({ mode: "serial" });
  test.skip(({ isMobile }) => isMobile, "one project drives the idle bench");

  test("the banner clears when the write ends, and the next job resets the log", async ({ page }) => {
    test.setTimeout(60_000);
    await page.goto(`${IDLE}/`);
    const banner = page.getByRole("alert", { name: "UART write in progress" });
    await expect(page.getByText("No job yet")).toBeVisible();
    await expect(banner).toHaveCount(0);

    expect(await ended(client(["on"]))).toBe(0);
    const write = client([
      "flash-execute",
      "flash-e2e-short",
      "--source",
      "s",
      "--target",
      "t",
      "--expected-plan-sha256",
      "00",
    ]);
    await expect(banner).toContainText("3 of 19 sectors verified");
    await expect(banner).toHaveCount(0, { timeout: 20_000 });
    await ended(write);

    const log = page.getByRole("log", { name: "Live log" });
    await expect(log).toContainText("Verified changed sectors: 19/19");
    await ended(client(["capture", "e2e-next", "--seconds", "1"]));

    // The new job's output replaces the write's, and the write fades out
    // of Activity rather than vanishing.
    const job = page.getByRole("region", { name: "Current job" });
    await expect(job.getByRole("heading", { name: "capture", level: 1 })).toBeVisible();
    await expect(log).not.toContainText("Verified changed sectors");
    const activity = page.getByRole("region", { name: "Activity" });
    const old = activity.getByRole("listitem").filter({ hasText: "flash-execute flash-e2e-short" });
    await expect(old).toContainText("cleared");
    expect(Number(await old.evaluate((row) => getComputedStyle(row).opacity))).toBeLessThan(1);
  });
});

test.describe("at phone width", () => {
  test.skip(({ isMobile }) => !isMobile, "phone layout only");

  /** Every matched element is at least a 44 px square, the touch target minimum. */
  async function expectThumbSized(targets: Locator) {
    // The polled views arrive after the first state.
    await expect(targets.first()).toBeVisible();
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
    await page.goto(`${WRITING}/`);

    const log = page.getByRole("log", { name: "Live log" });
    await expect(log).toContainText("Erasing bank 1 sector");
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

import type { CatalogEntry, ScenarioStats, Snapshot } from "@wellfed/api";
import { describe, expect, it } from "vitest";
import {
  includes,
  itestCommand,
  liveRun,
  missingFlags,
  nextRunName,
  passRate,
  runLabel,
  type RunOptions,
} from "./tests";

const catalog: CatalogEntry[] = [
  { name: "ota-update", about: "install", tier: "smoke", needs: [], baseline: true },
  { name: "requests", about: "ping", tier: "smoke", needs: [], baseline: false },
  {
    name: "settings-persist",
    about: "cold boot",
    tier: "standard",
    needs: [{ flag: "--power", why: "it cold-boots the feeder" }],
    baseline: false,
  },
  { name: "soak", about: "pings", tier: "soak", needs: [], baseline: false },
];

/** A scenario's stats with only the counts the test is about. */
function stats(pass: number, fail: number, skip: number): ScenarioStats {
  return { scenario: "requests", pass, fail, skip, median_seconds: null, last_failure: null, history: [], flips: 0 };
}

const options: RunOptions = {
  name: "itest-20261022-15",
  tier: "standard",
  only: [],
  power: false,
  build: false,
  destructive: false,
  keepGoing: false,
};

function snapshot(outcome: "running" | "pass", command = "itest"): Snapshot {
  return {
    now: 1000,
    history: [],
    rx: null,
    qa: null,
    script: null,
    current: {
      pid: 1,
      command,
      name: "itest-20261022-15",
      started: 900,
      finished: outcome === "running" ? null : 990,
      outcome,
      error: null,
      steps: [
        { label: "ota-update", state: "done", started: 900, finished: 960 },
        { label: "requests", state: "running", started: 960 },
        { label: "soak", state: "pending" },
      ],
      progress: null,
      capture: null,
      transfer: null,
      relay: {},
      lines: [],
      next: null,
      flash_tool: null,
      alive: true,
    },
  };
}

describe("live run", () => {
  it("joins a running suite's steps to the catalog with elapsed times", () => {
    const live = liveRun(snapshot("running"), catalog, 1000);
    expect(live?.running?.name).toBe("requests");
    expect(live?.scenarios.map((s) => s.elapsed)).toEqual([60, 40, null]);
    expect(live?.scenarios[1]?.about).toBe("ping");
  });

  it("is absent once the run finished, or for another command", () => {
    expect(liveRun(snapshot("pass"), catalog, 1000)).toBeNull();
    expect(liveRun(snapshot("running", "ota"), catalog, 1000)).toBeNull();
    expect(liveRun(null, catalog, 1000)).toBeNull();
  });
});

describe("run builder", () => {
  it("follows the suite's selection: tiers, --only and the baseline", () => {
    expect(catalog.filter((e) => includes(e, options)).map((e) => e.name)).toEqual([
      "ota-update",
      "requests",
      "settings-persist",
    ]);
    expect(catalog.filter((e) => includes(e, { tier: "smoke", only: ["soak"] })).map((e) => e.name)).toEqual([
      "ota-update",
      "soak",
    ]);
  });

  it("names the flags a scenario would be skipped without", () => {
    const persist = catalog[2];
    if (!persist) throw new Error("the catalog fixture has three entries");
    expect(missingFlags(persist, options)).toEqual(["--power"]);
    expect(missingFlags(persist, { ...options, power: true })).toEqual([]);
  });

  it("writes the command a developer would type", () => {
    expect(itestCommand(options)).toBe("wellfed-bench --ui itest itest-20261022-15");
    expect(itestCommand({ ...options, tier: "smoke", power: true, keepGoing: true })).toBe(
      "wellfed-bench --ui itest itest-20261022-15 --tier smoke --power --keep-going",
    );
    expect(itestCommand({ ...options, only: ["requests", "soak"], build: true })).toBe(
      "wellfed-bench --ui itest itest-20261022-15 --only requests,soak --build",
    );
  });

  it("suggests the next run name for today", () => {
    const today = new Date(2026, 9, 22);
    expect(nextRunName(["itest-20261022-09", "itest-20261022-14", "itest-20261021-30"], today)).toBe(
      "itest-20261022-15",
    );
    expect(nextRunName([], today)).toBe("itest-20261022-01");
  });
});

describe("history", () => {
  it("rates passes over runs that reached a verdict", () => {
    expect(passRate(stats(3, 1, 5))).toBe(0.75);
    expect(passRate(stats(0, 0, 2))).toBeNull();
  });

  it("labels run columns by their sequence number", () => {
    expect(runLabel("itest-20261022-07")).toBe("07");
    expect(runLabel("speaker-check")).toBe("chec");
  });
});

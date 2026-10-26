// The integration suite as the Tests view needs it: the live run joined to
// the catalog, and the command a developer would type next.

import type { CatalogEntry, ScenarioStats, Snapshot, Step, Tier } from "@wellfed/api";

export interface LiveScenario {
  name: string;
  about: string;
  step: Step;
  /** Seconds it has run, or ran; null while pending. */
  elapsed: number | null;
}

export interface LiveRun {
  name: string;
  outcome: string;
  started: number;
  finished: number | null;
  scenarios: LiveScenario[];
  /** The scenario running now, if any. */
  running: LiveScenario | null;
}

/**
 * The current bench command while it is an integration run in progress. A
 * finished one is shown from its record instead, in the run history.
 */
export function liveRun(snapshot: Snapshot | null, catalog: CatalogEntry[], now: number): LiveRun | null {
  const current = snapshot?.current;
  if (current?.command !== "itest" || current.outcome !== "running") return null;

  const scenarios = current.steps.map((step) => ({
    name: step.label,
    about: catalog.find((entry) => entry.name === step.label)?.about ?? "",
    step,
    elapsed: step.started ? (step.finished ?? (step.state === "running" ? now : step.started)) - step.started : null,
  }));
  return {
    name: current.name ?? "",
    outcome: current.outcome,
    started: current.started,
    finished: current.finished,
    scenarios,
    running: scenarios.find((s) => s.step.state === "running") ?? null,
  };
}

/** Passes out of runs that reached a verdict; skips are not attempts. */
export function passRate(stats: ScenarioStats): number | null {
  const ran = stats.pass + stats.fail;
  return ran ? stats.pass / ran : null;
}

export interface RunOptions {
  name: string;
  tier: Tier;
  only: string[];
  power: boolean;
  build: boolean;
  destructive: boolean;
  feeds: boolean;
  keepGoing: boolean;
}

/**
 * The `wellfed-bench itest` invocation for these options. The page only
 * writes it out: running it is a physical bench operation for the operator.
 */
export function itestCommand(options: RunOptions): string {
  const parts = ["wellfed-bench", "--ui", "itest", options.name || "NAME"];
  if (options.only.length) parts.push("--only", options.only.join(","));
  else if (options.tier !== "standard") parts.push("--tier", options.tier);
  if (options.build) parts.push("--build");
  if (options.power) parts.push("--power");
  if (options.destructive) parts.push("--destructive");
  if (options.feeds) parts.push("--feeds");
  if (options.keepGoing) parts.push("--keep-going");
  return parts.join(" ");
}

/** `itest-YYYYMMDD-NN`, one past the highest number used today. */
export function nextRunName(names: string[], today: Date): string {
  const day = `${today.getFullYear()}${String(today.getMonth() + 1).padStart(2, "0")}${String(today.getDate()).padStart(2, "0")}`;
  const prefix = `itest-${day}-`;
  const used = names
    .filter((name) => name.startsWith(prefix))
    .map((name) => Number.parseInt(name.slice(prefix.length), 10))
    .filter((n) => Number.isFinite(n));
  const next = (used.length ? Math.max(...used) : 0) + 1;
  return `${prefix}${String(next).padStart(2, "0")}`;
}

const TIER_RANK: Record<Tier, number> = { smoke: 0, standard: 1, soak: 2 };

/** Whether a run with these options takes the scenario on, as `select` in the suite does. */
export function includes(entry: CatalogEntry, options: Pick<RunOptions, "tier" | "only">): boolean {
  if (entry.baseline) return true;
  if (options.only.length) return options.only.includes(entry.name);
  return TIER_RANK[entry.tier] <= TIER_RANK[options.tier];
}

/** The flags a scenario needs that these options leave off: it would be skipped. */
export function missingFlags(entry: CatalogEntry, options: RunOptions): string[] {
  const on: Record<string, boolean> = {
    "--power": options.power,
    "--build": options.build,
    "--destructive": options.destructive,
    "--feeds": options.feeds,
  };
  return entry.needs.map((need) => need.flag).filter((flag) => !on[flag]);
}

/** A run's short column label: its sequence number, or the name's tail. */
export function runLabel(name: string): string {
  const tail = name.split("-").pop() ?? name;
  return /^\d{1,3}$/.test(tail) ? tail : tail.slice(0, 4);
}

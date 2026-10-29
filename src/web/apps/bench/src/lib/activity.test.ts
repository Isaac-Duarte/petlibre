import type { Activity, BenchState } from "@wellfed/api";
import { describe, expect, it } from "vitest";
import { FADE_SECONDS, activityRows, fade, track } from "./activity";
import { IDLE_CAPTURE, IDLE_FEEDER } from "./testing";

function state(activity: Activity[], end: number | null = null): BenchState {
  return {
    now: 100,
    server: { pid: 9, started: 0, phase: "serving", uart_port: true, stopping: null },
    job: {
      id: 3,
      label: "flash-execute u1",
      log: "private/bench-status/jobs/3-u1.log",
      accepted: 10,
      record: null,
      upcoming: [],
      cancel: "none",
      end: end === null ? null : { code: 0, error: null, finished: end },
    },
    activity,
    uart: { state: "idle" },
    relays: {},
    feeder: IDLE_FEEDER,
    capture: IDLE_CAPTURE,
    banks: { banks: {} },
    operator: { step: null, next: [], timers: [], evidence: [] },
  };
}

const flash: Activity = { kind: "flash", pid: 41, label: "update execute", started: 20 };

describe("activity", () => {
  it("lists the job and its children, newest first", () => {
    const rows = track([], activityRows(state([flash])), 30);

    expect(rows.map((r) => [r.kind, r.label, r.cleared])).toEqual([
      ["flash", "update execute", null],
      ["job", "flash-execute u1", null],
    ]);
  });

  it("keeps a cleared child, fading, until it has been gone a while", () => {
    const running = track([], activityRows(state([flash])), 30);
    const after = track(running, activityRows(state([], 50)), 50);

    const child = after.find((r) => r.kind === "flash");
    expect(child?.cleared).toBe(50);
    expect(after.find((r) => r.kind === "job")?.cleared).toBe(50);
    if (!child) throw new Error("the cleared child stays");
    expect(fade(child, 50)).toBeGreaterThan(fade(child, 50 + FADE_SECONDS / 2));

    // A later state keeps the first time it was missed, then drops it.
    expect(track(after, activityRows(state([], 50)), 60).find((r) => r.kind === "flash")?.cleared).toBe(50);
    expect(track(after, activityRows(state([], 50)), 50 + FADE_SECONDS).some((r) => r.kind === "flash")).toBe(false);
  });
});

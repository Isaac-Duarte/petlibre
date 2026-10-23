// The operator script: timed instructions counted from an anchor that
// arrives in a capture, as `script.json` gives them.

import type { ScriptStep } from "@petlibre/api";

export interface ScriptPosition {
  /** Seconds since the anchor arrived. */
  clock: number;
  index: number;
  current: ScriptStep | null;
  next: ScriptStep | null;
}

export function scriptPosition(steps: ScriptStep[], started: number, now: number): ScriptPosition {
  const clock = now - started;
  let index = -1;
  steps.forEach((step, i) => {
    if (clock >= step.at) index = i;
  });
  return {
    clock,
    index,
    current: index >= 0 ? (steps[index] ?? null) : null,
    next: steps[index + 1] ?? null,
  };
}

/** "1:05": a countdown, as an operator reads it at a glance. */
export function countdown(seconds: number): string {
  const s = Math.max(0, Math.ceil(seconds));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

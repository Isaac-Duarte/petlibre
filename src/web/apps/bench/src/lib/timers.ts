// The operator's timers in words: when each is due, what it expects, and
// whether the server has seen that arrive or marked it missed.

import type { Expect, Timer } from "@wellfed/api";
import { clockTime, duration } from "@wellfed/ui";

/** "in 4m 10s" until it is due, then "due 1m 2s ago". */
export function dueText(timer: Timer, now: number): string {
  const left = timer.due - now;
  return left > 0 ? `in ${duration(left)}` : `due ${duration(-left)} ago`;
}

export function expectText(expect: Expect): string {
  return expect.on === "line" ? `a line with ${expect.text}` : `a ${expect.kind} event`;
}

/** What became of the expectation, or `null` while it may still come. */
export function settledText(timer: Timer): string | null {
  if (timer.state.state === "seen") return `seen at ${clockTime(timer.state.at)}: ${timer.state.line}`;
  if (timer.state.state === "missed") return `missed: nothing by ${clockTime(timer.until)}`;
  return null;
}

/** The badge tone for a timer: seen, missed, or still open. */
export function timerTone(timer: Timer): "pass" | "fail" | "muted" {
  if (timer.state.state === "seen") return "pass";
  if (timer.state.state === "missed") return "fail";
  return "muted";
}

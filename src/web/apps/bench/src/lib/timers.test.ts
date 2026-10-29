import type { Timer } from "@wellfed/api";
import { describe, expect, it } from "vitest";
import { dueText, expectText, settledText, timerTone } from "./timers";

const timer: Timer = {
  label: "fire",
  started: 0,
  due: 300,
  until: 420,
  expect: { on: "line", text: "PLSCH1 fire" },
  state: { state: "waiting" },
};

describe("timers", () => {
  it("count down to the due time, then up from it", () => {
    expect(dueText(timer, 50)).toBe("in 4m 10s");
    expect(dueText(timer, 362)).toBe("due 1m 02s ago");
  });

  it("say what they expect and what became of it", () => {
    expect(expectText({ on: "line", text: "PLSCH1 fire" })).toBe("a line with PLSCH1 fire");
    expect(expectText({ on: "event", kind: "boot" })).toBe("a boot event");
    expect(settledText(timer)).toBeNull();
    expect(timerTone(timer)).toBe("muted");

    const seen: Timer = { ...timer, state: { state: "seen", at: 310, line: "PLSCH1 fire slot=0" } };
    expect(settledText(seen)).toMatch(/^seen at .*: PLSCH1 fire slot=0$/);
    expect(timerTone(seen)).toBe("pass");
    const missed: Timer = { ...timer, state: { state: "missed", at: 421 } };
    expect(settledText(missed)).toMatch(/^missed: nothing by /);
    expect(timerTone(missed)).toBe("fail");
  });
});

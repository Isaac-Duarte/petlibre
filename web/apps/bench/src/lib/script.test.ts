import { describe, expect, it } from "vitest";
import { countdown, scriptPosition } from "./script";

describe("operator script", () => {
  const steps = [
    { at: 0, text: "Hands clear" },
    { at: 30, text: "Move the probe to TP3" },
    { at: 60, text: "Done" },
  ];

  it("shows the step whose time has come and the next one", () => {
    const p = scriptPosition(steps, 100, 135);
    expect(p.current?.text).toBe("Move the probe to TP3");
    expect(p.next?.text).toBe("Done");
    expect(Math.round(p.clock)).toBe(35);
  });

  it("is before the first step until the anchor's time", () => {
    expect(scriptPosition(steps, 100, 99).current).toBeNull();
  });

  it("counts down in minutes and seconds", () => {
    expect(countdown(65)).toBe("1:05");
    expect(countdown(-3)).toBe("0:00");
  });
});

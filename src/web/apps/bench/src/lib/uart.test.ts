import { describe, expect, it } from "vitest";
import { holdsUart, uncountedText } from "./uart";

describe("the UART", () => {
  it("is held by the flash client while writing or in a loader session", () => {
    expect(holdsUart({ state: "writing", done: 0, total: 0 })).toBe(true);
    expect(holdsUart({ state: "loader" })).toBe(true);
    expect(holdsUart({ state: "capturing", path: null })).toBe(false);
    expect(holdsUart({ state: "idle" })).toBe(false);
    expect(holdsUart(null)).toBe(false);
  });

  it("says which writes never report a sector count", () => {
    const child = (label: string) => ({ kind: "flash" as const, pid: 1, label, started: 0 });

    expect(uncountedText([child("give-way execute")])).toBe("This write (give-way execute) reports no sector count");
    expect(uncountedText([child("stock-recovery execute")])).toContain("reports no sector count");
    expect(uncountedText([child("update execute")])).toBe("Waiting for the first sector count");
  });
});

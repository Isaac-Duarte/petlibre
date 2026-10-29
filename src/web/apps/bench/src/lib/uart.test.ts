import { describe, expect, it } from "vitest";
import { holdsUart, uncountedText } from "./uart";

describe("the UART", () => {
  it("is held by the flash client while writing or in a loader session", () => {
    expect(holdsUart({ state: "writing", done: 0, total: 0, counted: true })).toBe(true);
    expect(holdsUart({ state: "loader" })).toBe(true);
    expect(holdsUart({ state: "capturing", path: null })).toBe(false);
    expect(holdsUart({ state: "idle" })).toBe(false);
    expect(holdsUart(null)).toBe(false);
  });

  it("says when a write reports no sector count, as the server marks it", () => {
    expect(uncountedText({ state: "writing", done: 0, total: 0, counted: false })).toBe(
      "This write reports no sector count",
    );
    expect(uncountedText({ state: "writing", done: 0, total: 0, counted: true })).toBe(
      "Waiting for the first sector count",
    );
  });
});

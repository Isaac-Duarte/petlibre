import type { Ledger } from "@wellfed/api";
import { describe, expect, it } from "vitest";
import { checkText, confirmed, contentText } from "./banks";

const ledger: Ledger = {
  banks: {
    "0": {
      content: { kind: "stock" },
      source: "seed",
      session: null,
      at: 1,
      check: { state: "confirmed", by: "banner", at: 5 },
    },
    "1": {
      content: { kind: "build", id: "4d37c21a99887766" },
      source: "flash-execute",
      session: "u1",
      at: 2,
      check: { state: "disputed", by: "status", at: 6, seen: { kind: "build", id: "bb22" } },
    },
  },
};

describe("banks", () => {
  it("says what each bank holds and whether the feeder agrees", () => {
    expect(contentText({ kind: "build", id: "4d37c21a99887766" })).toBe("build 4d37c21a9988");
    expect(contentText({ kind: "invalid" })).toBe("no valid image");
    expect(checkText({ state: "unchecked" })).toBe("not reported since");
    expect(checkText({ state: "disputed", by: "status", at: 6, seen: { kind: "stock" } })).toBe(
      "disputed: the device API's status reported stock",
    );
  });

  it("names the newest bank the feeder confirmed", () => {
    expect(confirmed(ledger)?.[0]).toBe("0");
    expect(confirmed({ banks: {} })).toBeNull();
  });
});

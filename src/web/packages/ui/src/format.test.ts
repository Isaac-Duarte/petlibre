import { describe, expect, it } from "vitest";
import { ago, bytes, duration, eta, percent, shortHash } from "./format";
import { toneOf } from "./tone";

describe("format", () => {
  it("writes durations at bench precision", () => {
    expect(duration(9.4)).toBe("9s");
    expect(duration(245)).toBe("4m 05s");
    expect(duration(3780)).toBe("1h 03m");
    expect(duration(-2)).toBe("0s");
  });

  it("writes sizes, ages, hashes and percents", () => {
    expect(bytes(812)).toBe("812 B");
    expect(bytes(14_540)).toBe("14.2 KiB");
    expect(ago(100, 400)).toBe("5m");
    expect(shortHash("4d37c21a00112233aabb")).toBe("4d37c21a0011");
    expect(percent(1, 3)).toBe("33%");
    expect(percent(0, 0)).toBe("–");
  });

  it("estimates a finish only once the rate is known", () => {
    expect(eta(110, 100, 0, 10, 100)).toBe(200);
    expect(eta(101, 100, 0, 10, 100)).toBeNull();
    expect(eta(110, null, 0, 10, 100)).toBeNull();
  });
});

describe("tone", () => {
  it("reads meaning from the many ways reports spell a state", () => {
    expect(toneOf("verified_via_supplied_transports")).toBe("pass");
    expect(toneOf("read complete")).toBe("pass");
    expect(toneOf("stopped: loader NAK")).toBe("fail");
    expect(toneOf("running")).toBe("live");
    expect(toneOf("prepared")).toBe("warn");
    expect(toneOf("ended")).toBe("muted");
  });
});

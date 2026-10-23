import type { QaCheckView, QaView } from "@petlibre/api";
import { describe, expect, it } from "vitest";
import { guidance, subject } from "./qa";

function view(overrides: Partial<QaView> = {}): QaView {
  const checks: QaCheckView[] = (["optics", "power", "buttons", "indicators", "speaker"] as const).map((check) => ({
    check,
    state: "waiting",
    items: [],
  }));
  return { capture: "private/captures/qa", checks, prompt: null, passed: null, unreadable: 0, ...overrides };
}

function running(check: QaCheckView["check"]): QaView {
  const qa = view();
  qa.checks = qa.checks.map((c) => (c.check === check ? { ...c, state: "running" } : c));
  return qa;
}

describe("QA guidance", () => {
  it("keeps hands off while the unattended checks run", () => {
    expect(guidance(running("optics")).headline).toBe("Hands off");
    expect(guidance(running("power")).detail).toMatch(/Button prompts/);
    expect(guidance(view()).headline).toBe("Waiting for QA");
  });

  it("names the button, the light and the answer buttons", () => {
    const press = view({ prompt: { ask: "press", target: "mic", asked: null } });
    expect(guidance(press).headline).toBe("Press Mic");

    const look = view({ prompt: { ask: "look", target: "PA0", asked: null } });
    expect(guidance(look)).toEqual({
      headline: "Look at the Wi-Fi light",
      detail: "Take a full second. Lit: press Dispense. Dark: press Lock.",
    });

    const listen = view({ prompt: { ask: "listen", target: "test", asked: null } });
    expect(guidance(listen).headline).toBe("Listen for the test tone");
    expect(guidance(listen).detail).toMatch(/Wait until it ends/);
  });

  it("reports the result with the failed checks", () => {
    const qa = view({ passed: false });
    qa.checks = qa.checks.map((c) => (c.check === "indicators" ? { ...c, state: "fail" } : { ...c, state: "pass" }));
    expect(guidance(qa)).toEqual({
      headline: "QA failed",
      detail: "Failed: Indicators. Grade it with petlibre-audit qa.",
    });
  });

  it("falls back to the pin for an unknown indicator", () => {
    expect(subject("look", "PB9")).toBe("indicator PB9");
    expect(subject("press", "reset")).toBe("Reset");
  });
});

import { describe, expect, it } from "vitest";
import { classify, lastWait, scenarioSections } from "./log";

describe("classify", () => {
  it("colours commands, scenario starts and verdicts", () => {
    expect(classify("+ wellfed-flash read observe")).toBe("cmd");
    expect(classify("- requests: ping, health, clock sync")).toBe("mark");
    expect(classify("  requests: FAIL: No reply to ping within 20 s")).toBe("bad");
    expect(classify("  requests: pass")).toBe("ok");
    expect(classify("loader NAK at 0x1000")).toBe("warn");
    expect(classify("  Waiting for fresh health: no health received yet")).toBe("warn");
  });

  it("does not take healthy health lines or file names for failures", () => {
    expect(classify("  Health: build 196652d97ee9 on bank 1, seq 1, outputs stopped, faults none")).toBe("");
    expect(classify("Signed manifest for version 52 in private/verification/x-fault-v52.manifest")).toBe("");
    expect(classify("  UART: PLOTA1 error stage=hash")).toBe("bad");
  });

  it("judges the flash client's JSON by its fields", () => {
    expect(classify('{"status":"complete","error":null}')).toBe("ok");
    expect(classify('{"status":"stopped","error":"loader NAK"}')).toBe("bad");
    expect(classify('{"status":"running","error":null}')).toBe("");
  });
});

describe("scenario sections", () => {
  const lines = [
    "Hotspot up",
    "- ota-update: install the build under test",
    "  Offered version 3 (abc)",
    "  ota-update: pass",
    "- requests: ping, health",
    "  Waiting for fresh health: stale",
    "- request-auth: skipped, needs --power",
  ];

  it("splits the suite's notes by scenario", () => {
    const sections = scenarioSections(lines);
    expect([...sections.keys()]).toEqual(["ota-update", "requests", "request-auth"]);
    expect(sections.get("ota-update")).toHaveLength(3);
  });

  it("finds a running scenario's current wait, and none once it has moved on", () => {
    const sections = scenarioSections(lines);
    expect(lastWait(sections.get("requests"))).toBe("Waiting for fresh health: stale");
    expect(lastWait([...(sections.get("requests") ?? []), "  UART: PLFW1 health"])).toBeNull();
    expect(lastWait(sections.get("ota-update"))).toBeNull();
  });
});

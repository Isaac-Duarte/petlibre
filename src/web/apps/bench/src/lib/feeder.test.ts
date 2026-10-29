import type { FeederView } from "@wellfed/api";
import { describe, expect, it } from "vitest";
import {
  UNKNOWN,
  captureText,
  clockText,
  hotspotText,
  journalText,
  sessionText,
  startText,
  statusText,
} from "./feeder";
import { IDLE_CAPTURE, IDLE_FEEDER } from "./testing";

const unread = IDLE_FEEDER;

const read: FeederView = {
  ...unread,
  session: { state: "open", address: "10.42.0.20:7468", build: "4d37c21a99887766", since: 1 },
  status: {
    build: "4d37c21a99887766",
    bank: 1,
    uptime_ms: 5000,
    boot_count: 12,
    rssi: -48,
    clock_ms: 1_700_000_000_000,
    locked: false,
    last_event: 41,
    sessions: 1,
  },
  status_at: 2,
  clock: { skew_ms: -310, synced_at: null },
  journal: { seq: 41, epoch: 3, from: "event" },
  hotspot: true,
};

describe("feeder", () => {
  it("says what the session has not read as not known", () => {
    expect(sessionText(unread.session)).toBe("not tried yet");
    expect(statusText(unread)).toBe(UNKNOWN);
    expect(clockText(unread)).toBe(UNKNOWN);
    expect(journalText(unread)).toBe(UNKNOWN);
    expect(hotspotText(null)).toBe(UNKNOWN);
  });

  it("reads an open session's status, clock and journal", () => {
    expect(sessionText(read.session)).toMatch(/^open to 10\.42\.0\.20:7468 since /);
    expect(statusText(read)).toMatch(/^build 4d37c21a9988, bank 1, boot 12, -48 dBm, read /);
    expect(clockText(read)).toBe("-310 ms");
    expect(journalText(read)).toBe("event 41 in epoch 3 (from the event stream)");
    expect(hotspotText(true)).toBe("up");
  });

  it("names why a session is paused or failing", () => {
    expect(sessionText({ state: "paused", pause: { kind: "stock-install", job: 9 } })).toBe(
      "paused: job 9 (install) holds its own sessions",
    );
    expect(sessionText({ state: "paused", pause: { kind: "uart-job", job: 7, label: "flash-execute" } })).toBe(
      "paused: job 7 (flash-execute) holds the UART",
    );
    expect(sessionText({ state: "connecting", attempt: 2, error: "refused", retry: 3 })).toBe(
      "connecting, 2 failed (refused)",
    );
    expect(clockText({ ...read, status: read.status ? { ...read.status, clock_ms: null } : null })).toBe("invalid");
  });

  it("says where the capture stands and the last banner", () => {
    const capture = IDLE_CAPTURE;
    expect(captureText(capture)).toBe("not capturing");
    expect(startText(capture)).toBe(UNKNOWN);
    expect(captureText({ ...capture, segment: "private/captures/live-01" })).toBe(
      "capturing to private/captures/live-01",
    );
    expect(startText({ ...capture, start: { build: "4d37c21a99887766", bank: 0 } })).toBe("build 4d37c21a9988, bank 0");
  });
});

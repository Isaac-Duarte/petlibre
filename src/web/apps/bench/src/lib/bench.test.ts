import type { BenchState, Uart } from "@wellfed/api";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDLE_CAPTURE, IDLE_FEEDER } from "./testing";

const api = vi.hoisted(() => ({
  health: vi.fn(() => Promise.resolve(null)),
  sessions: vi.fn(() => Promise.resolve(null)),
  tests: vi.fn(() => Promise.resolve(null)),
}));
vi.mock("@wellfed/api", () => ({ bench: api, stream: vi.fn() }));

const { Bench } = await import("./bench.svelte");

function state(uart: Uart): BenchState {
  return {
    now: 100,
    server: { pid: 9, started: 0, phase: "serving", uart_port: true },
    job: null,
    activity: [],
    uart,
    relays: {},
    feeder: IDLE_FEEDER,
    capture: IDLE_CAPTURE,
    banks: { banks: {} },
    operator: { step: null, next: [], timers: [], evidence: [] },
  };
}

const calls = () => api.health.mock.calls.length + api.sessions.mock.calls.length + api.tests.mock.calls.length;

describe("the polled views", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("wait for the first state", async () => {
    const page = new Bench();

    await page.refresh();

    expect(calls()).toBe(0);
  });

  it("are never asked for while the flash client holds the UART", async () => {
    const page = new Bench();

    for (const uart of [{ state: "writing", done: 3, total: 19, counted: true }, { state: "loader" }] as const) {
      page.take(state(uart));
      await page.refresh();
      await page.poll("health");
    }
    expect(calls()).toBe(0);

    // It lets go: everything is read again at once.
    page.take(state({ state: "idle" }));
    await vi.waitFor(() => {
      expect(calls()).toBe(3);
    });
  });
});

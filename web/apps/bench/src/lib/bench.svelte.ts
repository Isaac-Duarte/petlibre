// Everything the page shows, in one reactive store: status, log and feeder
// arrive over the event stream; health, tests and sessions are polled, and
// re-read at once when a command finishes.

import { bench, stream } from "@wellfed/api";
import type { FeederView, HealthView, Sessions, Snapshot, TestsView } from "@wellfed/api";

/** The page keeps this many log lines for scrolling, filtering and copying. */
const LOG_LINES = 20_000;

class Bench {
  snapshot = $state.raw<Snapshot | null>(null);
  feeder = $state.raw<FeederView | null>(null);
  health = $state.raw<HealthView | null>(null);
  tests = $state.raw<TestsView | null>(null);
  sessions = $state.raw<Sessions | null>(null);
  lines = $state.raw<string[]>([]);
  connected = $state(false);
  /** The bench host's clock, ticking locally between snapshots. */
  now = $state(Date.now() / 1000);

  // Server clock minus ours, so timers agree with the bench host.
  #skew = 0;
  #outcome: string | null = null;

  start(): () => void {
    const close = stream({
      status: (snapshot) => {
        this.#skew = snapshot.now - Date.now() / 1000;
        this.snapshot = snapshot;
        const outcome = snapshot.current ? `${snapshot.current.started}:${snapshot.current.outcome}` : null;
        if (outcome !== this.#outcome) {
          this.#outcome = outcome;
          void this.refresh();
        }
      },
      logReset: () => (this.lines = []),
      log: (text) => {
        const added = text.replace(/\n$/, "").split("\n");
        const all = this.lines.concat(added);
        this.lines = all.length > LOG_LINES ? all.slice(all.length - LOG_LINES) : all;
      },
      feeder: (view) => (this.feeder = view),
      connected: (open) => (this.connected = open),
    });

    void this.refresh();
    const timers = [
      setInterval(() => (this.now = Date.now() / 1000 + this.#skew), 1000),
      setInterval(() => void this.poll("health"), 5_000),
      setInterval(() => void this.poll("sessions"), 10_000),
      setInterval(() => void this.poll("tests"), 10_000),
    ];
    return () => {
      close();
      timers.forEach(clearInterval);
    };
  }

  async refresh(): Promise<void> {
    await Promise.all([this.poll("health"), this.poll("sessions"), this.poll("tests")]);
  }

  async poll(view: "health" | "sessions" | "tests"): Promise<void> {
    try {
      if (view === "health") this.health = await bench.health();
      else if (view === "sessions") this.sessions = await bench.sessions();
      else this.tests = await bench.tests();
    } catch {
      // The stream's state already shows a lost bench; keep the last view.
    }
  }
}

export const app = new Bench();

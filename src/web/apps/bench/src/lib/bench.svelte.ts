// Everything the page shows, in one reactive store: the server's live
// state, the job's output, the evidence and the feeder arrive over the
// event stream; health, tests and sessions are polled, and re-read at once
// when a job ends.

import { bench, stream } from "@wellfed/api";
import type { BenchState, EvidenceView, FeederView, HealthView, Sessions, TestsView } from "@wellfed/api";
import { activityRows, track, type ActivityRow } from "./activity";

/** The page keeps this many log lines for scrolling, filtering and copying. */
const LOG_LINES = 20_000;

class Bench {
  state = $state.raw<BenchState | null>(null);
  evidence = $state.raw<EvidenceView | null>(null);
  feeder = $state.raw<FeederView | null>(null);
  health = $state.raw<HealthView | null>(null);
  tests = $state.raw<TestsView | null>(null);
  sessions = $state.raw<Sessions | null>(null);
  activity = $state.raw<ActivityRow[]>([]);
  lines = $state.raw<string[]>([]);
  connected = $state(false);
  /** The bench host's clock, ticking locally between states. */
  now = $state(Date.now() / 1000);

  // Server clock minus ours, so timers agree with the bench host.
  #skew = 0;
  #ended: string | null = null;

  start(): () => void {
    const close = stream({
      state: (state) => {
        this.#skew = state.now - Date.now() / 1000;
        this.state = state;
        this.activity = track(this.activity, activityRows(state), state.now);
        const job = state.job;
        const ended = job ? `${String(job.id)}:${job.end ? "end" : "run"}` : null;
        if (ended !== this.#ended) {
          this.#ended = ended;
          void this.refresh();
        }
      },
      evidence: (view) => (this.evidence = view),
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

  /** The running job: one that has not ended. */
  get running(): boolean {
    return !!this.state?.job && !this.state.job.end;
  }

  async refresh(): Promise<void> {
    await Promise.all([this.poll("health"), this.poll("sessions"), this.poll("tests")]);
  }

  async poll(view: "health" | "sessions" | "tests"): Promise<void> {
    // A UART write is left alone: these views read evidence files.
    if (this.state?.uart.state === "writing" && this.health && this.sessions && this.tests) return;
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

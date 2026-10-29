// Everything the page shows, in one reactive store: the server's live
// state, the job's output, the evidence and the stock server's view arrive over the
// event stream; health, tests and sessions are polled, and re-read at once
// when a job ends or the flash client lets go of the UART.

import { bench, stream } from "@wellfed/api";
import type { BenchState, EvidenceView, HealthView, StockView, Sessions, TestsView } from "@wellfed/api";
import { activityRows, track, type ActivityRow } from "./activity";
import { holdsUart } from "./uart";

/** The page keeps this many log lines for scrolling, filtering and copying. */
const LOG_LINES = 20_000;

export type Polled = "health" | "sessions" | "tests";

export class Bench {
  state = $state.raw<BenchState | null>(null);
  evidence = $state.raw<EvidenceView | null>(null);
  stock = $state.raw<StockView | null>(null);
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
  // What last called for a refresh: the job and whether it ended, and
  // whether the flash client held the UART.
  #moment: string | null = null;

  start(): () => void {
    const close = stream({
      state: (state) => {
        this.take(state);
      },
      evidence: (view) => (this.evidence = view),
      logReset: () => (this.lines = []),
      log: (text) => {
        const added = text.replace(/\n$/, "").split("\n");
        const all = this.lines.concat(added);
        this.lines = all.length > LOG_LINES ? all.slice(all.length - LOG_LINES) : all;
      },
      stock: (view) => (this.stock = view),
      connected: (open) => (this.connected = open),
    });

    // The polled views wait for the first state: until then the page does
    // not know whether the flash client holds the UART.
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

  /** A new state from the server. */
  take(state: BenchState): void {
    this.#skew = state.now - Date.now() / 1000;
    this.state = state;
    this.activity = track(this.activity, activityRows(state), state.now);

    const job = state.job;
    const moment = `${job ? `${String(job.id)}:${job.end ? "end" : "run"}` : "none"}:${String(holdsUart(state.uart))}`;
    if (moment !== this.#moment) {
      this.#moment = moment;
      void this.refresh();
    }
  }

  /** The running job: one that has not ended. */
  get running(): boolean {
    return !!this.state?.job && !this.state.job.end;
  }

  async refresh(): Promise<void> {
    await Promise.all([this.poll("health"), this.poll("sessions"), this.poll("tests")]);
  }

  /**
   * Re-read one polled view, unless the state is unknown yet or the flash
   * client holds the UART: those views read files, and the page keeps what
   * it showed until the write ends.
   */
  async poll(view: Polled): Promise<void> {
    if (!this.state || holdsUart(this.state.uart)) return;
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

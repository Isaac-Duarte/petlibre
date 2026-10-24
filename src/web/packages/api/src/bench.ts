// The wellfed-bench page's API: JSON views and one event stream, all
// read-only. Paths are relative so the page works under any host and port
// the bench serves it on (loopback, or a tailnet address).

import type { FeederView, HealthView, RunView, SessionView, Sessions, Snapshot, TestsView } from "./bench.gen";

async function get<T>(path: string): Promise<T | null> {
  const response = await fetch(path, { cache: "no-store" });
  if (response.status === 404) return null;
  if (!response.ok) throw new Error(`GET ${path}: ${response.status}`);
  return (await response.json()) as T;
}

export const bench = {
  status: () => get<Snapshot>("api/status"),
  health: () => get<HealthView>("api/health"),
  sessions: () => get<Sessions>("api/sessions"),
  session: (name: string) => get<SessionView>(`api/sessions/${encodeURIComponent(name)}`),
  tests: () => get<TestsView>("api/tests"),
  run: (name: string) => get<RunView>(`api/tests/${encodeURIComponent(name)}`),
  feeder: () => get<FeederView>("api/feeder"),
};

export interface StreamHandlers {
  /** The status snapshot, sent whenever it changes. */
  status(snapshot: Snapshot): void;
  /** A new command started, or the log shrank: drop what was shown. */
  logReset(): void;
  /** Whole new lines of the current command's log. */
  log(text: string): void;
  /** The local server's view of the feeder, sent when it changes. */
  feeder(view: FeederView): void;
  /** Whether the stream is connected; the browser reconnects on its own. */
  connected(open: boolean): void;
}

/** Follow `api/events`; returns the function that closes it. */
export function stream(handlers: StreamHandlers): () => void {
  const source = new EventSource("api/events");
  // Named events arrive as MessageEvent, but addEventListener types them as Event.
  const data = (event: Event) => (event instanceof MessageEvent && typeof event.data === "string" ? event.data : "");

  source.addEventListener("open", () => {
    handlers.connected(true);
  });
  source.addEventListener("error", () => {
    handlers.connected(false);
  });
  source.addEventListener("status", (e) => {
    handlers.status(JSON.parse(data(e)) as Snapshot);
  });
  source.addEventListener("log-reset", () => {
    handlers.logReset();
  });
  source.addEventListener("log", (e) => {
    handlers.log(data(e));
  });
  source.addEventListener("feeder", (e) => {
    handlers.feeder(JSON.parse(data(e)) as FeederView);
  });

  return () => {
    source.close();
  };
}

// The wellfed-bench page's API: JSON views and one event stream, read-only,
// and the answer to a job's question, the one thing the page sends. Paths
// are relative so the page works under any host and port the bench serves
// it on (loopback, or a tailnet address).

import type {
  Answer,
  AnswerError,
  AnswerRequest,
  BenchState,
  EvidenceView,
  StockView,
  HealthView,
  RunView,
  SessionView,
  Sessions,
  TestsView,
} from "./bench.gen";

/**
 * The bench refused a view that reads files because the flash client holds
 * the UART (503): it is deferred until the write ends, not missing.
 */
export class Deferred extends Error {
  constructor(path: string) {
    super(`GET ${path}: deferred until the UART write ends`);
    this.name = "Deferred";
  }
}

async function get<T>(path: string): Promise<T | null> {
  const response = await fetch(path, { cache: "no-store" });
  if (response.status === 404) return null;
  if (response.status === 503) throw new Deferred(path);
  if (!response.ok) throw new Error(`GET ${path}: ${response.status}`);
  return (await response.json()) as T;
}

export const bench = {
  state: () => get<BenchState>("api/state"),
  evidence: () => get<EvidenceView>("api/evidence"),
  health: () => get<HealthView>("api/health"),
  sessions: () => get<Sessions>("api/sessions"),
  session: (name: string) => get<SessionView>(`api/sessions/${encodeURIComponent(name)}`),
  tests: () => get<TestsView>("api/tests"),
  run: (name: string) => get<RunView>(`api/tests/${encodeURIComponent(name)}`),
  stock: () => get<StockView>("api/stock"),
};

/** The bench server did not take an answer; `refusal` says why, when it could be read. */
export class AnswerRefused extends Error {
  constructor(
    readonly refusal: AnswerError | null,
    readonly status: number,
  ) {
    super(`POST answer: ${String(status)}`);
    this.name = "AnswerRefused";
  }
}

const TOKEN_KEY = "wellfed-bench-answer-token";

/**
 * The answer token, from the link the bench console prints
 * (`#token=...`), or else from this tab's `sessionStorage`, where a token
 * from the link is kept so a reload or a discarded phone tab still
 * answers. The link is put back without it, out of the address bar and
 * the routes. Storage may be refused (a private window); the token then
 * lasts as long as the page. Without one the page shows questions but
 * cannot answer them.
 */
const token: string | null = (() => {
  if (typeof window === "undefined") return null;
  const found = /^#token=([0-9a-f]+)$/.exec(window.location.hash)?.[1] ?? null;
  if (found) {
    window.history.replaceState(null, "", window.location.pathname + window.location.search);
    try {
      window.sessionStorage.setItem(TOKEN_KEY, found);
    } catch {
      // Kept in memory only.
    }
    return found;
  }
  try {
    return window.sessionStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
})();

/** Whether this page was opened with the console's link, so it may answer. */
export function canAnswer(): boolean {
  return token !== null;
}

/**
 * Answer job `job`'s question `seq`. Sent once and never retried: the
 * installer reads answers in order, so a resent one could answer the next
 * question unseen. A refusal throws {@link AnswerRefused}.
 */
export async function answer(job: number, seq: number, reply: Answer): Promise<void> {
  const body: AnswerRequest = { seq, answer: reply };
  const response = await fetch(`api/jobs/${String(job)}/answer`, {
    method: "POST",
    headers: { "content-type": "application/json", "x-bench-token": token ?? "" },
    body: JSON.stringify(body),
    cache: "no-store",
  });
  if (response.ok) return;
  const refusal = (await response.json().catch(() => null)) as AnswerError | null;
  throw new AnswerRefused(refusal, response.status);
}

export interface StreamHandlers {
  /** The bench server's live state, on connecting and on every change. */
  state(state: BenchState): void;
  /** The history and the latest capture, sent when they change. */
  evidence(view: EvidenceView): void;
  /** Another job's output follows (its number as the data, since a browser drops an empty event): drop what was shown. */
  logReset(): void;
  /** Whole new lines of the job's output. */
  log(text: string): void;
  /** The local stock server's view of the feeder, sent when it changes. */
  stock(view: StockView): void;
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
  source.addEventListener("state", (e) => {
    handlers.state(JSON.parse(data(e)) as BenchState);
  });
  source.addEventListener("evidence", (e) => {
    handlers.evidence(JSON.parse(data(e)) as EvidenceView);
  });
  source.addEventListener("log-reset", () => {
    handlers.logReset();
  });
  source.addEventListener("log", (e) => {
    handlers.log(data(e));
  });
  source.addEventListener("stock", (e) => {
    handlers.stock(JSON.parse(data(e)) as StockView);
  });

  return () => {
    source.close();
  };
}

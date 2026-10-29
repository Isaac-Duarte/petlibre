// Who holds what on the bench, as a list that remembers: the server's state
// names only what runs now, so an entry that clears stays a while, fading,
// for an operator who looked away to see what just ran.

import type { ActivityKind, BenchState } from "@wellfed/api";

/** How long a cleared entry stays in the list. */
export const FADE_SECONDS = 60;

export interface ActivityRow {
  key: string;
  kind: "job" | ActivityKind;
  label: string;
  /** The job's number or the child's pid. */
  id: number;
  entered: number;
  /** When it cleared; null while it runs. */
  cleared: number | null;
}

/** What the state says holds the bench now: the job, then its children. */
export function activityRows(state: BenchState): ActivityRow[] {
  const rows: ActivityRow[] = [];
  const job = state.job;
  if (job) {
    rows.push({
      key: `job-${String(job.id)}`,
      kind: "job",
      label: job.label,
      id: job.id,
      entered: job.accepted,
      cleared: job.end?.finished ?? null,
    });
  }
  for (const child of state.activity) {
    rows.push({
      key: `${child.kind}-${String(child.pid)}-${String(child.started)}`,
      kind: child.kind,
      label: child.label,
      id: child.pid,
      entered: child.started,
      cleared: null,
    });
  }
  return rows;
}

/**
 * The list after a new state: what runs now, and what cleared within
 * {@link FADE_SECONDS}, stamped when it was first missed. Newest first.
 */
export function track(previous: ActivityRow[], current: ActivityRow[], now: number): ActivityRow[] {
  const keys = new Set(current.map((row) => row.key));
  const gone = previous
    .filter((row) => !keys.has(row.key))
    .map((row) => ({ ...row, cleared: row.cleared ?? now }))
    .filter((row) => now - row.cleared < FADE_SECONDS);
  return [...current, ...gone].sort((a, b) => b.entered - a.entered);
}

/** How opaque a row is drawn: 1 while it runs, fading to 0.25 as it leaves. */
export function fade(row: ActivityRow, now: number): number {
  if (row.cleared === null) return 1;
  const left = 1 - (now - row.cleared) / FADE_SECONDS;
  return 0.25 + 0.5 * Math.max(0, Math.min(1, left));
}

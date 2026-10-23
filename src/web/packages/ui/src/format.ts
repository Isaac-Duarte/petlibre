// Small, pure formatters shared by the petlibre pages. Times are Unix
// seconds, as the Rust views record them.

/** "4m 05s", "12s", "1h 03m": elapsed time at the precision a bench needs. */
export function duration(seconds: number): string {
  const s = Math.max(0, Math.round(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const r = s % 60;
  if (h) return `${h}h ${String(m).padStart(2, "0")}m`;
  if (m) return `${m}m ${String(r).padStart(2, "0")}s`;
  return `${r}s`;
}

/** Wall-clock time of day, "14:03:22". */
export function clockTime(t: number, seconds = true): string {
  return new Date(t * 1000).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    ...(seconds ? { second: "2-digit" } : {}),
    hourCycle: "h23",
  });
}

/** Full local date and time, for titles. */
export function fullTime(t: number): string {
  return new Date(t * 1000).toLocaleString();
}

/** "now", "5m", "3h", "2d": how long ago, compactly. */
export function ago(t: number, now: number): string {
  const s = now - t;
  if (s < 60) return "now";
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}

/** A hash or build ID cut to twelve characters, as the bench prints them. */
export function shortHash(hash: string | null | undefined, length = 12): string {
  return hash ? hash.slice(0, length) : "";
}

/** "812 B", "14.2 KiB", "3.41 MiB". */
export function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1 << 20) return `${(n / 1024).toFixed(1)} KiB`;
  return `${(n / (1 << 20)).toFixed(2)} MiB`;
}

/** Percent with no more precision than the data has. */
export function percent(part: number, whole: number): string {
  if (!whole) return "–";
  const p = (100 * part) / whole;
  return `${p >= 10 || p === 0 ? p.toFixed(0) : p.toFixed(1)}%`;
}

/**
 * Finish time by linear extrapolation from a baseline `(t0, v0)` to `(now, v)`;
 * null until the rate is known, so an ETA never shows from one sample.
 */
export function eta(now: number, t0: number | null, v0: number, v: number, total: number): number | null {
  if (!t0 || v <= v0 || now - t0 < 3) return null;
  return now + ((total - v) * (now - t0)) / (v - v0);
}

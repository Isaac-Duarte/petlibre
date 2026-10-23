/** What a state means, which is all a component colours by. */
export type Tone = "pass" | "fail" | "live" | "warn" | "muted";

/**
 * The tone of a state word. Reports spell outcomes many ways ("read
 * complete", "verified_via_supplied_transports"), so this reads meaning,
 * not exact values.
 */
export function toneOf(word: string | null | undefined): Tone {
  const w = String(word ?? "").toLowerCase();
  if (/^(running|started|live)/.test(w)) return "live";
  if (/stopped|fail|refused|interrupted|fault|silent/.test(w)) return "fail";
  if (/verified|complete|^pass|^done|^ok|^up$/.test(w)) return "pass";
  if (/prepared|planned|stale|warn/.test(w)) return "warn";
  return "muted";
}

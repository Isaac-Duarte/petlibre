// The server's feeder session and receive-only capture in words. The
// server holds the only device API session on the bench, so what it last
// read is what the page can say; anything it has not read stays "not known".

import type { CaptureView, FeederView, Session } from "@wellfed/api";
import { clockTime, shortHash } from "@wellfed/ui";

export const UNKNOWN = "not known yet";

export function sessionText(session: Session): string {
  switch (session.state) {
    case "idle":
      return "not tried yet";
    case "unconfigured":
      return `no session: ${session.error}`;
    case "paused":
      return `paused: ${pauseText(session)}`;
    case "connecting": {
      const tries = session.attempt === 0 ? "connecting" : `connecting, ${session.attempt} failed`;
      return session.error ? `${tries} (${session.error})` : tries;
    }
    case "open":
      return `open to ${session.address} since ${clockTime(session.since, false)}`;
  }
}

function pauseText(session: Extract<Session, { state: "paused" }>): string {
  const pause = session.pause;
  switch (pause.kind) {
    case "uart-job":
      return `job ${pause.job} (${pause.label}) holds the UART`;
    case "install":
      return "an install push holds the feeder's session";
    case "stock-ota":
      return `job ${pause.job} (stock-ota) holds its own sessions`;
    case "power-off":
      return "relay 1 is off";
    case "stopping":
      return "the bench server is stopping";
  }
}

/** The tone of the session's dot: open, trying, or not there. */
export function sessionTone(session: Session): "pass" | "warn" | "muted" {
  if (session.state === "open") return "pass";
  if (session.state === "connecting") return "warn";
  return "muted";
}

export function statusText(feeder: FeederView): string {
  const status = feeder.status;
  if (!status) return UNKNOWN;
  const read = feeder.status_at === null ? "" : `, read ${clockTime(feeder.status_at, false)}`;
  return `build ${shortHash(status.build)}, bank ${status.bank}, boot ${status.boot_count}, ${status.rssi} dBm${read}`;
}

export function clockText(feeder: FeederView): string {
  if (feeder.status?.clock_ms === null) return "invalid";
  const skew = feeder.clock.skew_ms;
  if (skew === null) return UNKNOWN;
  const off = `${skew >= 0 ? "+" : ""}${skew} ms`;
  const synced = feeder.clock.synced_at;
  return synced === null ? off : `${off}, set ${clockTime(synced, false)}`;
}

export function journalText(feeder: FeederView): string {
  const journal = feeder.journal;
  if (!journal) return UNKNOWN;
  const epoch = journal.epoch === null ? "" : ` in epoch ${journal.epoch}`;
  return `event ${journal.seq}${epoch} (from the ${journal.from === "status" ? "status" : "event stream"})`;
}

export function hotspotText(hotspot: boolean | null): string {
  if (hotspot === null) return UNKNOWN;
  return hotspot ? "up" : "down";
}

export function captureText(capture: CaptureView): string {
  if (capture.segment) return `capturing to ${capture.segment}`;
  if (capture.failure) return `stopped: ${capture.failure}`;
  return "not capturing";
}

export function startText(capture: CaptureView): string {
  const start = capture.start;
  if (!start) return UNKNOWN;
  return start.bank === null
    ? `build ${shortHash(start.build)}`
    : `build ${shortHash(start.build)}, bank ${start.bank}`;
}

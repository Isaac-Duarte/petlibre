// What a freshly started server holds for the feeder and the capture, for
// the unit tests' states.

import type { CaptureView, FeederView } from "@wellfed/api";

export const IDLE_FEEDER: FeederView = {
  session: { state: "idle" },
  status: null,
  status_at: null,
  clock: { skew_ms: null, synced_at: null },
  journal: null,
  hotspot: null,
  log: [],
  other_sessions: 0,
};

export const IDLE_CAPTURE: CaptureView = { segment: null, failure: null, lines: [], start: null, update: null };

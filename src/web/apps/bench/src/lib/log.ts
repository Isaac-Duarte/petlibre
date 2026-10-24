// How the live log is read: line colours, and the integration suite's
// scenario sections.

import type { LineKind } from "@wellfed/ui";

/** `- ota-update: install the build…` starts a scenario's section. */
const SCENARIO_START = /^- ([a-z0-9][a-z0-9-]*): /;

// The flash client's one-line JSON summaries carry "error":null on success,
// so judge those by their fields rather than by the words in them.
function classifyJson(line: string): LineKind | null {
  let value: unknown;
  try {
    value = JSON.parse(line);
  } catch {
    return null;
  }
  if (!value || typeof value !== "object") return null;
  const status = "status" in value && typeof value.status === "string" ? value.status : "";
  if ("error" in value && value.error) return "bad";
  if (/^(stopped|failed|refused|interrupted)/.test(status)) return "bad";
  if (/^(complete|verified|prepared)/.test(status)) return "ok";
  return "";
}

// Health lines report a stopped motor and an empty fault list in these
// words; neither is a problem.
const BENIGN = /\b(outputs stopped|faults none|error=null|"error":null)\b/gi;

/** A whole word: not part of a path or name such as `-fault-v52.manifest`. */
function word(words: string): RegExp {
  return new RegExp(`(?<![\\w./-])(?:${words})(?![\\w/-])`, "i");
}
const BAD = word("stopped|refused|faults?|failed|FAIL|errors?|not confirmed|interrupted|silent");
const WARN = word("NAKs?");
const OK = word("complete|completed|verified|confirmed|captured|prepared|pass");

export function classify(line: string): LineKind {
  if (line.startsWith("+ ")) return "cmd";
  if (SCENARIO_START.test(line)) return "mark";
  if (line.startsWith("{")) {
    const kind = classifyJson(line);
    if (kind !== null) return kind;
  }
  const text = line.replace(BENIGN, "");
  if (BAD.test(text)) return "bad";
  if (WARN.test(text) || /^\s+Waiting for /.test(line)) return "warn";
  if (OK.test(text)) return "ok";
  return "";
}

/** The suite's notes, split by scenario: what each one printed while it ran. */
export function scenarioSections(lines: string[]): Map<string, string[]> {
  const sections = new Map<string, string[]>();
  let current: string[] | null = null;
  for (const line of lines) {
    const start = SCENARIO_START.exec(line);
    if (start?.[1]) {
      current = [];
      sections.set(start[1], current);
    }
    current?.push(line);
  }
  return sections;
}

/** The newest "Waiting for …" note in a section, while it still applies. */
export function lastWait(section: string[] | undefined): string | null {
  if (!section) return null;
  for (let i = section.length - 1; i >= 0; i--) {
    const line = section[i]?.trim() ?? "";
    if (line.startsWith("Waiting for ")) return line;
    if (line.startsWith("UART:") || line.endsWith(": pass")) return null;
  }
  return null;
}

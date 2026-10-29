// The bank ledger in words: what the bench believes each flash bank holds,
// by whose word, and whether the feeder's own report agrees.

import type { Bank, Check, Content, Entry, Ledger, Source } from "@wellfed/api";
import { shortHash } from "@wellfed/ui";

export const BANKS: readonly Bank[] = ["0", "1"];

export function contentText(content: Content): string {
  if (content.kind === "build") return `build ${shortHash(content.id)}`;
  if (content.kind === "stock") return "stock";
  return "no valid image";
}

const SOURCE: Record<Source, string> = {
  seed: "seeded from a full read",
  "flash-execute": "written over UART",
  "give-way": "given way",
  "ota-confirmed": "installed over the air, confirmed",
  "ota-fell-back": "installed over the air, fell back",
  "stock-ota": "stock's own over-the-air install",
};

export function sourceText(source: Source): string {
  return SOURCE[source];
}

export function checkText(check: Check): string {
  if (check.state === "unchecked") return "not reported since";
  const by = check.by === "banner" ? "the start banner" : "the device API's status";
  if (check.state === "confirmed") return `confirmed by ${by}`;
  return `disputed: ${by} reported ${contentText(check.seen)}`;
}

/** The newest entry the feeder itself confirmed, if any. */
export function confirmed(ledger: Ledger): [Bank, Entry] | null {
  let best: [Bank, Entry, number] | null = null;
  for (const bank of BANKS) {
    const entry = ledger.banks[bank];
    if (entry?.check.state === "confirmed" && (!best || entry.check.at > best[2])) {
      best = [bank, entry, entry.check.at];
    }
  }
  return best ? [best[0], best[1]] : null;
}

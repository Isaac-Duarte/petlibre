// What the UART's state means for the page: whether the flash client holds
// it, and what the write banner can say about its progress.

import type { Activity, Uart } from "@wellfed/api";

/**
 * The flash client holds the UART: writing flash, or in a loader session
 * that may be about to. The page asks for no view that reads files then;
 * the server would defer it anyway.
 */
export function holdsUart(uart: Uart | null | undefined): boolean {
  return uart?.state === "writing" || uart?.state === "loader";
}

/** The writes whose flash client prints no sector count. */
const UNCOUNTED = ["give-way execute", "stock-recovery execute"];

/** The banner's line for a write that has no count to show yet. */
export function uncountedText(activity: Activity[]): string {
  const writer = activity.find((child) => child.kind === "flash" && UNCOUNTED.includes(child.label));
  return writer ? `This write (${writer.label}) reports no sector count` : "Waiting for the first sector count";
}

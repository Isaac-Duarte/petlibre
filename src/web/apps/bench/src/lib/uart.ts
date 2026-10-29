// What the UART's state means for the page: whether the flash client holds
// it, and what the write banner can say about its progress.

import type { Uart } from "@wellfed/api";

/**
 * The flash client holds the UART: writing flash, or in a loader session
 * that may be about to. The page asks for no view that reads files then;
 * the server would defer it anyway.
 */
export function holdsUart(uart: Uart | null | undefined): boolean {
  return uart?.state === "writing" || uart?.state === "loader";
}

/** The banner's line for a write with no count to show yet. */
export function uncountedText(uart: Extract<Uart, { state: "writing" }>): string {
  return uart.counted ? "Waiting for the first sector count" : "This write reports no sector count";
}

// What the guided QA run asks of the operator, in words for someone standing
// at the feeder rather than reading UART records.

import type { QaAsk, QaCheckView, QaView } from "@wellfed/api";

/** The front-panel indicators by the pin QA drives (docs/hardware). */
const INDICATORS: Record<string, string> = {
  PA0: "Wi-Fi",
  PA2: "low food",
  PA4: "lock",
  PB22: "warning (red)",
};

const BUTTONS: Record<string, string> = {
  reset: "Reset",
  mic: "Mic",
  dispense: "Dispense",
  lock: "Lock",
};

export const CHECK_NAMES: Record<QaCheckView["check"], string> = {
  optics: "Optics",
  power: "Power",
  buttons: "Buttons",
  indicators: "Indicators",
  speaker: "Speaker",
};

/** Every look and listen prompt is answered with these two buttons. */
export const YES = "Dispense";
export const NO = "Lock";

/** How long the firmware waits for an answer before it records a timeout. */
export const ANSWER_SECONDS = 30;

export function indicatorName(pin: string): string {
  const name = INDICATORS[pin];
  return name ? `${name} light` : `indicator ${pin}`;
}

export function buttonName(button: string): string {
  return BUTTONS[button] ?? button;
}

/** "Wi-Fi light", "Reset", "test tone": what an item or prompt is about. */
export function subject(ask: QaAsk, target: string): string {
  if (ask === "press") return buttonName(target);
  if (ask === "look") return indicatorName(target);
  return target === "test" ? "test tone" : `${target} tune`;
}

export interface Guidance {
  /** The instruction, large. */
  headline: string;
  /** How to answer, or what happens next. */
  detail: string;
}

/** What to do now. */
export function guidance(qa: QaView): Guidance {
  if (qa.passed !== null) {
    const failed = qa.checks.filter((c) => c.state === "fail").map((c) => CHECK_NAMES[c.check]);
    return qa.passed
      ? { headline: "QA passed", detail: "Every check passed. Grade it with wellfed-audit qa." }
      : { headline: "QA failed", detail: `Failed: ${failed.join(", ")}. Grade it with wellfed-audit qa.` };
  }

  const prompt = qa.prompt;
  if (prompt) {
    if (prompt.ask === "press") {
      return { headline: `Press ${buttonName(prompt.target)}`, detail: "Press only that button, once." };
    }
    if (prompt.ask === "look") {
      return {
        headline: `Look at the ${indicatorName(prompt.target)}`,
        detail: `Take a full second. Lit: press ${YES}. Dark: press ${NO}.`,
      };
    }
    return {
      headline: `Listen for the ${subject("listen", prompt.target)}`,
      detail: `Wait until it ends (1 s). Heard it: press ${YES}. Silence: press ${NO}.`,
    };
  }

  const running = qa.checks.find((c) => c.state === "running")?.check;
  switch (running) {
    case "optics":
      return { headline: "Hands off", detail: "Optics is measuring (about 2 minutes). Guided checks follow." };
    case "power":
      return { headline: "Hands off", detail: "Power is sampling (5 s). Button prompts come next." };
    case undefined:
      return { headline: "Waiting for QA", detail: "The run starts with optics after boot." };
    default:
      return { headline: "Get ready", detail: `${CHECK_NAMES[running]}: the next prompt is coming.` };
  }
}

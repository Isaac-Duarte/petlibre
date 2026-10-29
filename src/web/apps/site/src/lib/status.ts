// What the page claims about the project. It is written by hand, in owner
// words: the stock column from docs/devices/plaf103/stock/README.md (static
// analysis and emulation of stock 1.1.15), the Wellfed column and its stages
// from docs/roadmap.md. Update it when the roadmap's status changes, and
// never claim more than the roadmap does.

/** How far along a Wellfed behavior is, shown in words beside it. */
export type Stage = "available" | "bench" | "progress" | "planned";

export const STAGE_LABELS: Record<Stage, string> = {
  available: "Available now",
  bench: "Works in testing",
  progress: "In progress",
  planned: "Planned",
};

export const UPDATED = "2026-10-27";

export interface Step {
  title: string;
  body: string;
  stage: Stage;
}

/** The owner's path from a stock feeder to a running replacement. */
export const STEPS: Step[] = [
  {
    title: "Install over the air",
    body: "The feeder's own updater installs Wellfed, and the same route takes it back to stock if you change your mind. No screwdriver, no soldering.",
    stage: "bench",
  },
  {
    title: "Pair it with your phone",
    body: "On first boot the feeder offers Bluetooth setup. You give it your Wi-Fi and pair while standing next to it, so nobody else can. It works from the command line today, and the phone app is next.",
    stage: "progress",
  },
  {
    title: "Feed on your terms",
    body: "Schedules, portions and history live on the feeder itself. Use the app, or run a small server at home if the whole household wants in.",
    stage: "planned",
  },
];

export interface Comparison {
  topic: string;
  stock: string;
  wellfed: string;
  stage: Stage;
}

export const COMPARISON: Comparison[] = [
  {
    topic: "Who's in control",
    stock:
      "The vendor's cloud. There's no local control, so every feed and setting travels over the internet and back.",
    wellfed: "You. Your phone, laptop or home server talks straight to the feeder on your Wi-Fi.",
    stage: "bench",
  },
  {
    topic: "Account",
    stock: "Setup ties the feeder to a vendor account.",
    wellfed: "No account. You pair with the feeder itself.",
    stage: "bench",
  },
  {
    topic: "On your network",
    stock: "Talks to the vendor's servers without encryption.",
    wellfed: "Encrypted end to end, between the feeder and the devices you pair.",
    stage: "bench",
  },
  {
    topic: "Feeding history",
    stock: "Each record is deleted from the feeder as soon as the cloud has it.",
    wellfed: "At least seven days kept on the feeder, readable without a server.",
    stage: "planned",
  },
  {
    topic: "Time zones",
    stock: "Whole hours only, so a half-hour zone moves every meal by 30 minutes.",
    wellfed: "Full time zones with daylight saving, so meals stay at the time you set.",
    stage: "planned",
  },
  {
    topic: "Updates",
    stock: "Pushed through the vendor's cloud.",
    wellfed: "Signed releases that install only when you say so, and roll back on their own if they fail.",
    stage: "bench",
  },
  {
    topic: "Source code",
    stock: "Closed.",
    wellfed: "Open, under the MIT license.",
    stage: "available",
  },
];

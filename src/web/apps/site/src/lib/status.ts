// What the page claims about the project. It is written by hand, in owner
// words: the stock column from docs/devices/plaf103/stock/README.md (static
// analysis and emulation of stock 1.1.15), the Petlibre column and its stages
// from FIRMWARE_BOARD.md and FEATURE_BOARD.md. Update it when a card's
// acceptance evidence changes, and never claim more than the boards do.

/** How far along a Petlibre behavior is, shown in words beside it. */
export type Stage = "available" | "bench" | "progress" | "planned";

export const STAGE_LABELS: Record<Stage, string> = {
  available: "Available now",
  bench: "Works on the bench",
  progress: "In progress",
  planned: "Planned",
};

export const UPDATED = "2026-10-24";

export interface Step {
  title: string;
  body: string;
  stage: Stage;
}

/** The owner's path from a stock feeder to a running replacement. */
export const STEPS: Step[] = [
  {
    title: "Install over the air",
    body: "The feeder's own updater installs Petlibre, and the same path takes it back to stock. No screwdriver, no soldering.",
    stage: "bench",
  },
  {
    title: "Join your Wi-Fi",
    body: "On first boot the feeder offers Bluetooth setup. The Android app gives it your network and pairs with it.",
    stage: "progress",
  },
  {
    title: "Feed on your terms",
    body: "Schedules, portions and history live on the feeder. Use the app, or add a server you host for the household.",
    stage: "planned",
  },
];

export interface Comparison {
  topic: string;
  stock: string;
  petlibre: string;
  stage: Stage;
}

export const COMPARISON: Comparison[] = [
  {
    topic: "Who sends commands",
    stock: "The vendor's cloud. The feeder has no local control, so every feed and setting arrives over the internet.",
    petlibre: "Your phone or your own server, straight to the feeder on your Wi-Fi.",
    stage: "planned",
  },
  {
    topic: "Account",
    stock: "Setup binds the feeder to a vendor account.",
    petlibre: "None. You pair your phone with the feeder itself.",
    stage: "progress",
  },
  {
    topic: "On your network",
    stock: "Plain, unencrypted MQTT to the vendor's servers.",
    petlibre: "Encrypted end to end with the Noise protocol.",
    stage: "progress",
  },
  {
    topic: "Feeding history",
    stock: "Each record is deleted from the feeder once the cloud receives it.",
    petlibre: "At least seven days kept on the feeder, readable without a server.",
    stage: "planned",
  },
  {
    topic: "Time zones",
    stock: "Whole hours only, so a half-hour zone moves every meal by 30 minutes.",
    petlibre: "Full time zones with daylight saving, so meals stay at the time you set.",
    stage: "planned",
  },
  {
    topic: "Updates",
    stock: "Pushed through the vendor's cloud.",
    petlibre: "Signed releases that install only when you approve them.",
    stage: "planned",
  },
  {
    topic: "Source code",
    stock: "Closed.",
    petlibre: "Open, under the MIT license.",
    stage: "available",
  },
];

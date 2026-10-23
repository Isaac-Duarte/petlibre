# Feature parity board

This board tracks the owner-visible behavior the replacement firmware needs to match stock 1.1.15 on the PLAF103: schedules, feeding, alerts, panel, sound and power. [FIRMWARE_BOARD.md](FIRMWARE_BOARD.md) tracks what the SDK and hardware can do safely (peripherals, supervision, storage, transport, OTA and the feeding guarantees); this board tracks what the device does with them, and when. For bench state and the next task, start with [RESUME.md](RESUME.md).

## Current status

- **Done:** none.
- **In progress:** none.
- **Critical path to parity:** FW30 → FW16 → FW17 → FW18 → FP03. Nothing a pet depends on works before FW17 can feed.
- **Startable without motion:** FP01, FP02, FP04 (host-tested against the snapshot format), FP09, FP11 (ready: FW20's button service is done), and the reporting halves of FP12 and FP14.

## Using the board

Cards use the FIRMWARE_BOARD workflow (**Planned → Ready → In progress → Bench check → Done**), evidence labels and carried-items rule. Card status never authorizes flashing, motion or a stock round trip.

A feature card sequences behavior through the SDK; it never relaxes a FIRMWARE_BOARD guarantee. Every feed goes through the FW16/FW17 feeding owner, with its journal reservation, no-replay rule and stop guarantees, and every state change goes through a signed request. A feature that needs a new SDK capability gets a FIRMWARE_BOARD card first.

**Scope.** Parity means the device's own functions, not the vendor app or cloud. Out of scope: the vendor app and cloud, binding, upsell and care plans, camera and motion switches (other models), wire compatibility with the stock app, and recording through the on-board microphone (a CLI upload is enough). The stock behavior column comes from static analysis of the 1.1.15 image, the vendor app and captured traffic; fields shared across the vendor's models are not confirmed for the PLAF103 until FP01 says so.

**v1 time policy.** There is no battery-backed RTC in use, so the clock is invalid after every cold boot ([clock](docs/firmware/clock.md)). For v1 the feeder needs the local server after a cold boot: no schedule runs until the server syncs the clock, and occurrences missed meanwhile follow FW21's missed-meal rule. Keeping time across a power cut with no server is deferred.

## Groundwork

| Card | Status | Evidence | Stock behavior | Prerequisites | Finish line |
| --- | --- | --- | --- | --- | --- |
| **FP01 Stock feature inventory** | Planned | — | — | — | A static pass over 1.1.15 and the vendor app pins, for every FP card, the stock limits (plans, portions per feed, portion size), defaults (desiccant period, lock timeouts, volume), button holds and battery-mode behavior. Each item is a recorded finding or an explicit unknown; nothing is guessed. Results go in [stock behavior](docs/firmware/stock-behavior.md). |
| **FP02 Owner control CLI** | Planned | — | The vendor app | FW08, FW25 | `petlibre-local` subcommands cover every FP setting and action through signed requests: feed, schedules, settings, history and status. This stands in for the app, and FP16 reuses the same interface. |

## Feeding

| Card | Status | Evidence | Stock behavior | Prerequisites | Finish line |
| --- | --- | --- | --- | --- | --- |
| **FP03 Feeding schedules** | Planned | — | `FEEDING_PLAN_SERVICE`: several plans, each with a time, portions, repeat days, an enable flag and a meal call | FW15, FW17, FW18, FW21, FP02 | Create, edit, enable, disable and delete plans from the CLI, within stock's limits. Plans persist and each occurrence runs exactly once through the feeding owner, with no server needed once the clock is valid. Cover DST, clock jumps, overlapping plans, restart and interrupted-meal reconciliation. After a cold boot nothing runs until the server syncs the clock, and missed occurrences follow FW21's rule. |
| **FP04 Import the stock schedule** | Planned | — | The server already holds the captured plan snapshot | FP03 | Moving from stock to the replacement keeps the owner's meals: the captured snapshot converts to replacement plans, and a plan that cannot convert is refused by name, never silently dropped. |
| **FP05 Manual feed** | Planned | — | The Dispense button and a remote feed of N portions; `GRAIN_OUTPUT_EVENT` start and end with expected and actual counts | FW17, FW20 | A button press and a remote request go through the same owner with stock's portion rules, and cannot double-feed. Each feed reports requested and delivered portions (outlet beam). |
| **FP06 Feeding history** | Planned | — | Feed and error records (`GRAIN_OUTPUT_EVENT`, `ERROR_EVENT`) | FW15, FW17 | A bounded on-device log of each feed (source, time, requested, delivered, result) that survives reboots and reads from the CLI. An uncertain feed reads as uncertain. |

## Sensors and alerts

| Card | Status | Evidence | Stock behavior | Prerequisites | Finish line |
| --- | --- | --- | --- | --- | --- |
| **FP07 Food level** | Planned | — | `surplusGrain` (low or empty) | FW13, FW18 | Food level from the food beam, with stock's states. An empty hopper shows in status, and a feed attempted while empty ends with an explicit result. |
| **FP08 Jam and outlet faults** | Planned | — | `barnDoorError`, `ERROR_EVENT` | FW19 | Jams, a blocked outlet and sensor faults reach status and history as specific faults, with stock's follow-up (retry count, skipping the meal) or a documented difference. |
| **FP09 Desiccant reminder** | Planned | — | `changeDesiccantFrequency`, `remainingDesiccantDays`, a reset | FW15, FW21 | A settable period, a remaining-days count that survives reboots, a reset from the CLI and a due state in status. Record whether it lives in firmware or on the server. |
| **FP10 Meal reminder** | Planned | — | `feedingPlanAdvanceNoticeTime` | FP03 | An advance event before each scheduled meal, sent to the server (and FP16) instead of a phone notification. |

## Panel, sound and power

| Card | Status | Evidence | Stock behavior | Prerequisites | Finish line |
| --- | --- | --- | --- | --- | --- |
| **FP11 Child lock** | Ready | — | `childLockSwitch`, auto-lock and unlock durations | FW20 | Lock and unlock by button hold and from the CLI, relocking after stock's timeout. A locked panel refuses feeds and setting changes but keeps recovery access. |
| **FP12 Lights and sound settings** | Planned | — | `lightSwitch`, `soundSwitch`, `volume` | FW23, FW24, FW25 | Each setting persists and behaves as stock: which indicator states show when lights are off, mute, volume steps and quiet windows. |
| **FP13 Meal call** | Planned | — | `enableAudio`, `audioUrl`; the Mic button records on stock | FW24, FP03 | The owner uploads a clip from the CLI, and it plays before each scheduled meal whose plan enables it, through FW24's playback. Audio never delays or blocks a feed (FW17 guarantee). Recording through the on-board microphone is out of scope. |
| **FP14 Battery and power** | Planned | — | `powerType`, `electricQuantity`, `enableBatteryPercent` | FW14, FW26 | Adapter or battery and a battery percentage on stock's curve. Battery-mode behavior matches stock (which radios stay up, whether schedules run), and low battery has a defined result. |
| **FP15 Reset button** | Planned | — | Reset button holds: Wi-Fi and/or factory reset | FW20, FW25 | Each hold does what stock does and clears exactly what stock clears; recovery access survives. |

## Integration and acceptance

| Card | Status | Evidence | Stock behavior | Prerequisites | Finish line |
| --- | --- | --- | --- | --- | --- |
| **FP16 Home Assistant integration** | Planned | — | — | FP02, FP03, FP05, FP08, FW25 | After standalone behavior works, add discovery, availability, sensors, settings and feed commands over the FP02 interface. Reconnects and automation replay cannot duplicate a feed, and feeding continues without Home Assistant. |
| **FP17 Parity soak** | Planned | — | — | FP03–FP15, FW27 | A defined multi-day run on schedules alone, with power cuts, server outages and a battery stretch. Every meal is accounted for exactly once, and the result is compared with stock in the same scenarios. FW29 waits on this card. |

## Status at a glance

✅ Done · 🚧 In progress · 🧪 Bench check · 🟢 Ready · ⬜ Planned. The cards above are authoritative.

| Card | Status |
| --- | --- |
| FP01 Stock feature inventory | ⬜ Planned |
| FP02 Owner control CLI | ⬜ Planned |
| FP03 Feeding schedules | ⬜ Planned |
| FP04 Import the stock schedule | ⬜ Planned |
| FP05 Manual feed | ⬜ Planned |
| FP06 Feeding history | ⬜ Planned |
| FP07 Food level | ⬜ Planned |
| FP08 Jam and outlet faults | ⬜ Planned |
| FP09 Desiccant reminder | ⬜ Planned |
| FP10 Meal reminder | ⬜ Planned |
| FP11 Child lock | 🟢 Ready |
| FP12 Lights and sound settings | ⬜ Planned |
| FP13 Meal call | ⬜ Planned |
| FP14 Battery and power | ⬜ Planned |
| FP15 Reset button | ⬜ Planned |
| FP16 Home Assistant integration | ⬜ Planned |
| FP17 Parity soak | ⬜ Planned |

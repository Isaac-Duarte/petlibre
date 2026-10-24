# Feature parity board

This board tracks the owner-visible behavior the replacement firmware needs to match stock 1.1.15 on the PLAF103: schedules, feeding, alerts, panel, sound and power. [FIRMWARE_BOARD.md](FIRMWARE_BOARD.md) tracks what the SDK and hardware can do safely (peripherals, supervision, storage, transport, OTA and the feeding guarantees); this board tracks what the device does with them, and when. For bench state and the next task, start with [RESUME.md](RESUME.md).

## Current status

- **Done:** FP01 (stock feature inventory), FP11 (child lock).
- **In progress:** none.
- **Critical path to parity:** FW30 → FW16 → FW17 → FW18 → FP03. Nothing a pet depends on works before FW17 can feed.
- **Startable without motion:** FP04's static pass on stock's plan records, FP09, and the reporting halves of FP12 and FP14.

## Using the board

Cards use the FIRMWARE_BOARD workflow (**Planned → Ready → In progress → Bench check → Done**), evidence labels and carried-items rule. Card status never authorizes flashing, motion or a stock round trip.

A feature card sequences behavior through the SDK; it never relaxes a FIRMWARE_BOARD guarantee. Every feed goes through the FW16/FW17 feeding owner, with its journal reservation, no-replay rule and stop guarantees, and every state change goes through a signed request. A feature that needs a new SDK capability gets a FIRMWARE_BOARD card first.

**Scope.** Parity means the device's own functions, not the vendor app or cloud. Out of scope: the vendor app and cloud, binding, upsell and care plans, camera and motion switches (other models), wire compatibility with the stock app, and recording through the on-board microphone (a CLI upload is enough). The stock behavior column comes from static analysis of the 1.1.15 image, the vendor app and captured traffic; fields shared across the vendor's models are not confirmed for the PLAF103 until FP01 says so.

**Time policy.** There is no battery-backed RTC in use, so the clock is invalid after every cold boot ([clock](docs/firmware/clock.md)). The feeder needs no server: any one time source (NTP, the app or the server) makes the clock valid, and it stays valid until its error could exceed a set bound (FW46); with no valid clock, scheduled meals hold and manual feeds still work. Until FW46 lands, only the server syncs. After a cold boot no schedule runs until a source syncs, and occurrences missed meanwhile follow FW21's missed-meal rule.

**Server optional.** Every card here works with the app alone on the LAN; the server adds long history and release fetching (FP18). See [product architecture](docs/firmware/product-architecture.md).

## Groundwork

| Card | Status | Evidence | Stock behavior | Prerequisites | Finish line |
| --- | --- | --- | --- | --- | --- |
| **FP01 Stock feature inventory** | **Done** | **Static (Oct 24):** a pass over 1.1.15 (settings defaults, feed, jam, outlet, record ring, sound and light windows, meal call, power, battery and OTA gates) and the vendor app (portion cap, plan count, desiccant, volume slider), combined with the earlier plan, manual-feed, panel, audio and Wi-Fi emulations and values the app logged for one unit. Every card from FP02 to FP19 has a finding or an explicit unknown: portion mass, the desiccant period range, the meal reminder's unit, volume-to-gain mapping, the quiet-window time base the app sends, `autoChangeMode`/`autoThreshold`, and what the factory restore clears ([table](docs/devices/plaf103/stock/README.md#feature-limits-and-defaults-fp01)). | Stock keeps desiccant, meal reminders, notifications and history in the app and cloud | — | A static pass over 1.1.15 and the vendor app pins, for every FP card, the stock limits (plans, portions per feed, portion size), defaults (desiccant period, lock timeouts, volume), button holds and battery-mode behavior. Each item is a recorded finding or an explicit unknown; nothing is guessed. Results go in [stock behavior](docs/devices/plaf103/stock/README.md). |
| **FP02 Owner control CLI** | In progress | **Physical (Oct 24):** `wellfed setup` (FW45) pairs over BLE. On the device API (FW47) it runs `status`, `settings` (writes against the generation just read), `lock`, `wifi`, `time` (with the system zone's next change), `events` (with `--follow` for live events and samples), `invite`, `join`, `clients`, `revoke`, `reboot` and `recover`; all but `recover`, `wifi` and `reboot` ran against the feeder ([device API](docs/firmware/device-api.md)). **Host (Oct 24, simulation):** its tokio connection moved to the shared `wellfed-connect`, which sends a keepalive so `events --follow` outlives the feeder's 90 s idle limit. Every command ran against `wellfed-feeder-sim`, a host feeder serving the device API over TCP from the feeder's own server, panel, pairing and stores, and its tests cover `recover`, `wifi` and `reboot` there. Feed and plans wait for FW16/FW17 and FP03 | The vendor app | FW45, FW47 | The `wellfed` CLI ([tools roadmap](docs/development/tools-roadmap.md)) covers every FP setting and action over the device API through `wellfed-client`: pair, feed, schedules, settings, history and status. This stands in for the app (FP19) until it exists. |

## Feeding

| Card | Status | Evidence | Stock behavior | Prerequisites | Finish line |
| --- | --- | --- | --- | --- | --- |
| **FP03 Feeding schedules** | Planned | — | `FEEDING_PLAN_SERVICE`: at most 10 plans, each with a time, portions, repeat days, an enable flag and a meal call; fields unchecked, no catch-up after power-off, whole-hour time zones ([FP01](docs/devices/plaf103/stock/README.md#feature-limits-and-defaults-fp01)) | FW15, FW17, FW18, FW21, FP02 | Create, edit, enable, disable and delete plans from the CLI, within stock's limits. Plans persist and each occurrence runs exactly once through the feeding owner, with no server needed once the clock is valid. Cover DST, clock jumps, overlapping plans, restart and interrupted-meal reconciliation. After a cold boot nothing runs until a time source syncs the clock, and missed occurrences follow FW21's rule. |
| **FP04 Import the stock schedule** | Planned | — | Stock keeps plan records in flash (`0x7d0000`–`0x7d7000`); their format is not yet recovered | FP03 | Moving from stock to the replacement keeps the owner's meals: before promotion, the bank-0 recovery role uploads stock's plan sectors with its bank-1 backup, `wellfed-stock` decodes them, and they convert to replacement plans. A plan that cannot convert is refused by name, never silently dropped. Needs a static pass on the record format first. |
| **FP05 Manual feed** | Planned | — | Dispense gives one portion; a remote feed takes `grainNum` as one unchecked byte (0–255); the app caps portions from a cloud field (48 observed); `GRAIN_OUTPUT_EVENT` start and end with expected and actual counts ([FP01](docs/devices/plaf103/stock/README.md#feature-limits-and-defaults-fp01)) | FW17, FW20 | A button press and a remote request go through the same owner with stock's portion rules, and cannot double-feed. Each feed reports requested and delivered portions (outlet beam). |
| **FP06 Feeding history** | Planned | — | An upload queue, not a history: two 50-record flash rings (feeds, errors), each record deleted once the cloud acks it ([FP01](docs/devices/plaf103/stock/README.md#feature-limits-and-defaults-fp01)) | FW15, FW17 | A bounded on-device log of each feed (source, time, requested, delivered, result) that survives reboots and reads from the CLI. An uncertain feed reads as uncertain. |

## Sensors and alerts

| Card | Status | Evidence | Stock behavior | Prerequisites | Finish line |
| --- | --- | --- | --- | --- | --- |
| **FP07 Food level** | Planned | — | `surplusGrain`, a boolean from the food beam (20-reading clear debounce); feeds are not refused when empty ([FP01](docs/devices/plaf103/stock/README.md#feature-limits-and-defaults-fp01)) | FW13, FW18 | Food level from the food beam, with stock's states. An empty hopper shows in status, and a feed attempted while empty ends with an explicit result. |
| **FP08 Jam and outlet faults** | Planned | — | Motor jam: three reversals, then the feed ends (code 2031, no retry). Outlet blocked: pause, resume within about 10 min or end (code 2008). `barnDoorError` is cloud-side ([FP01](docs/devices/plaf103/stock/README.md#feature-limits-and-defaults-fp01)) | FW19 | Jams, a blocked outlet and sensor faults reach status and history as specific faults, with stock's follow-up (retry count, skipping the meal) or a documented difference. |
| **FP09 Desiccant reminder** | Planned | — | Cloud only, not in firmware: `changeDesiccantFrequency` 14 and `remainingDesiccantDays` (goes negative) observed, reset through a cloud endpoint ([FP01](docs/devices/plaf103/stock/README.md#feature-limits-and-defaults-fp01)) | FW15, FW21 | A settable period, a remaining-days count that survives reboots, a reset from the CLI and a due state in status. Record whether it lives in firmware or on the server. |
| **FP10 Meal reminder** | Planned | — | Cloud only: `feedingPlanAdvanceNoticeTime` 5 or null observed; unit unknown ([FP01](docs/devices/plaf103/stock/README.md#feature-limits-and-defaults-fp01)) | FP03 | An advance event before each scheduled meal, sent to the server (and FP16) instead of a phone notification. |

## Panel, sound and power

| Card | Status | Evidence | Stock behavior | Prerequisites | Finish line |
| --- | --- | --- | --- | --- | --- |
| **FP11 Child lock** | **Done** | **Physical (Oct 23, owner pressing and watching PA4):** `feeder` v93 (`fp11-panel-01`) booted locked; in `fp11-panel-03` Dispense and Mic were refused, a 5.5 s Lock hold unlocked, the panel relocked after 30 s idle, and signed `lock` requests unlocked and relocked it (`source=remote`), with PA4 following each change. **Host and build (Oct 23):** `wellfed-feeder`'s `panel` keeps stock's rules (Lock held 5 s toggles, 30 s idle relock, locked panel refuses Dispense, Mic and the Reset hold) and starts locked; the `feeder` firmware shows it on PA4 and takes a signed `lock` request (`wellfed-local` API). Policy, owner, mailbox, parser and API host-tested; the image builds with its link, RAM-park and stack audits (loop task 960 B). Stock's rules are static analysis and emulation ([stock behavior](docs/devices/plaf103/stock/README.md#panel-buttons-and-child-lock)). `disableHardwareButton` is not implemented (FP15). | The PLAF103 has no `childLock*` fields; stock locks the panel with a 5 s Lock hold and relocks after 30 s idle. `disableHardwareButton` is separate | FW20 | Lock and unlock by button hold and from the CLI, relocking after stock's timeout. A locked panel refuses feeds and setting changes but keeps recovery access. |
| **FP12 Lights and sound settings** | Planned | — | `lightSwitch`, `soundSwitch` on and `volume` 50 by default; volume 0 mutes; an aging type selects always on or an inclusive start–end window ([FP01](docs/devices/plaf103/stock/README.md#feature-limits-and-defaults-fp01)) | FW23, FW24, FW25 | Each setting persists and behaves as stock: which indicator states show when lights are off, mute, volume steps and quiet windows. |
| **FP13 Meal call** | Planned | — | `enableAudio`, `audioUrl`; plays `audioTimes` times before a plan's feed when sound is allowed, never blocking it; the Mic button records on stock ([FP01](docs/devices/plaf103/stock/README.md#feature-limits-and-defaults-fp01)) | FW24, FP03 | The owner uploads a clip from the CLI, and it plays before each scheduled meal whose plan enables it, through FW24's playback. Audio never delays or blocks a feed (FW17 guarantee). Recording through the on-board microphone is out of scope. |
| **FP14 Battery and power** | Planned | — | `powerType` 1/2/3, `electricQuantity` on a 1650–2250 ADC curve. On battery stock drops Wi-Fi after 5 s, still runs meals, refuses OTA, and suspends below 0 % ([FP01](docs/devices/plaf103/stock/README.md#feature-limits-and-defaults-fp01)) | FW14, FW26 | Adapter or battery and a battery percentage on stock's curve. Battery-mode behavior matches stock (which radios stay up, whether schedules run), and low battery has a defined result. |
| **FP15 Reset button** | In progress | **Physical (Oct 24, `feeder` v107, owner pressing):** a Reset hold on an unlocked panel forgets every paired client and takes a new identity (`fw45-pair-04`), which survives a cold boot; a locked panel refuses the hold and the keys stay. Settings and Wi-Fi are kept for now, until stock's restore is known. The recovery window (Lock then Dispense held, unlocked panel) stays open to a new owner after a reset (FW45). | One Reset hold (about 5 s) starts the factory restore; no Wi-Fi-only hold. What it clears is unknown ([FP01](docs/devices/plaf103/stock/README.md#feature-limits-and-defaults-fp01)) | FW20, FW25 | Each hold does what stock does and clears exactly what stock clears; recovery access survives. |

## Integration and acceptance

| Card | Status | Evidence | Stock behavior | Prerequisites | Finish line |
| --- | --- | --- | --- | --- | --- |
| **FP16 Home Assistant integration** | Planned (paused by owner, Oct 23) | — | — | FP03, FP05, FP07, FP08, FW47 | Paused; the route (ESPHome API on the feeder, FW49, or MQTT discovery through the server) is chosen when it resumes ([options](docs/firmware/product-architecture.md#home-assistant)). Either way reconnects and automation replay cannot duplicate a feed, and feeding continues without Home Assistant. |
| **FP18 Server mirror** | Planned | — | The vendor cloud: history, remote control | FP06, FP08, FW47 | A new `wellfed-server` crate (shared pieces extracted from `wellfed-local` and the bench) mirrors the feeder's events through FW47's cursor with no gaps across its own outages, records live samples while connected, and keeps both raw, forever by default, with CSV and JSON export and import. It holds pet profiles and food info, shows the derived views, and fetches releases for the owner to approve. Stopping it changes nothing on the feeder. Notifications and remote access are deferred. Details: [data](docs/firmware/product-architecture.md#data). |
| **FP19 Android app** | Planned | — | The vendor app | FW45, FW47 | A Flutter Android app over the sans-IO `wellfed-client` core, released as a GitHub APK. First a spike against a host-simulated feeder: BLE setup, NSD discovery and a Noise session on a real phone. Then setup and pairing, invites, several feeders, feed, schedules, settings, history and status, plus server history when a server exists, and the stock installer. Every action goes through the device API; nothing bypasses the feeding owner. |
| **FP20 Web foundations** | **Done** | **Host check (Oct 24, worktree `standards-fp20`):** `make check-web` passed: ESLint (`strict-type-checked`, 0 warnings), Prettier, the license check (184 packages), svelte-check (0 errors, 0 warnings), Vitest in `@wellfed/tokens` (7: every named pair at AA in both themes, generated-file drift, malformed input), `@wellfed/ui` (4) and the bench (20), Clippy and `cargo test` for `wellfed-bench --features ui`, and the bench's 14 Playwright tests on desktop and phone with their assertions unchanged. `tokens.json` has theme roles for light and a graphite dark theme, state colors, spacing, type, radii, focus, elevation and motion; `@wellfed/tokens` generates the CSS and refuses a pair under its minimum; `@wellfed/ui` wraps bits-ui as `Dialog`, the bench's one interactive widget (the run drawer); the bench uses only token names. No hardware involved. | — | — | Step 1 of the [web UI design](docs/web/README.md#build-order): `assets/brand/tokens.json` gains state colors, spacing and a type scale, and a generator writes the CSS with a contrast test that fails below WCAG AA in either theme; `@wellfed/ui` wraps bits-ui for every interactive widget; the bench page moves to the Midnight tokens; ESLint, Prettier and the dependency policy run in `make check-web`. Done when `make check-web` passes with the bench on the new tokens and the bench's Playwright suite unchanged in what it asserts. |
| **FP21 Server web UI** | Planned | — | The vendor app | FP18, FP20, FW47 | Steps 2–6 of the [web UI design](docs/web/README.md#build-order): the SvelteKit app embedded in `wellfed-server`, with feed, status, schedules, history and charts, pets and food, and admin. Done when every screen passes Playwright against the real server and the simulated feeder on desktop and phone, axe-core finds nothing on any route in either theme, and the feeding-safety tests (double press, cut mid-feed, retry with the same key) each end with exactly one feed. |
| **FP17 Parity soak** | Planned | — | — | FP03–FP15, FW27 | A defined multi-day run on schedules alone, with power cuts, server outages and a battery stretch. Every meal is accounted for exactly once, and the result is compared with stock in the same scenarios. FW29 waits on this card. |

## Status at a glance

✅ Done · 🚧 In progress · 🧪 Bench check · 🟢 Ready · ⬜ Planned. The cards above are authoritative.

| Card | Status |
| --- | --- |
| FP01 Stock feature inventory | ✅ Done |
| FP02 Owner control CLI | 🚧 In progress |
| FP03 Feeding schedules | ⬜ Planned |
| FP04 Import the stock schedule | ⬜ Planned |
| FP05 Manual feed | ⬜ Planned |
| FP06 Feeding history | ⬜ Planned |
| FP07 Food level | ⬜ Planned |
| FP08 Jam and outlet faults | ⬜ Planned |
| FP09 Desiccant reminder | ⬜ Planned |
| FP10 Meal reminder | ⬜ Planned |
| FP11 Child lock | ✅ Done |
| FP12 Lights and sound settings | ⬜ Planned |
| FP13 Meal call | ⬜ Planned |
| FP14 Battery and power | ⬜ Planned |
| FP15 Reset button | 🚧 In progress |
| FP16 Home Assistant integration | ⬜ Planned (paused) |
| FP17 Parity soak | ⬜ Planned |
| FP18 Server mirror | ⬜ Planned |
| FP19 Android app | ⬜ Planned |
| FP20 Web foundations | ✅ Done |
| FP21 Server web UI | ⬜ Planned |

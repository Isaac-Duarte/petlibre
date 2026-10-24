# Stock behavior and local protocol

Findings apply to the inspected PLAF103 stock **1.1.15** image and research unit. Other models/releases may differ. Instruction-level behavior is distinguished from physical acceptance in the [hardware guide](../board.md).

## Network protocol

The observed feeder uses plaintext MQTT on TCP 1883 with device-specific credentials. Its initial CONNECT uses legacy MQTT 3.1 (`MQIsdp`); the server normalizes that CONNECT for an MQTT 3.1.1 broker and forces a clean session. It preserves the credentials, client identifier, keepalive, and later packets. Topic access stays within the configured device namespace.

Topics have the form `dl/PLAF103/<device-id>/<group>/<event>/post` or `/sub`. The local server handles startup, attributes, heartbeat, clock, binding, captured plan replies, and manual feeding. Consult the Rust protocol tests for complete synthetic wire examples. The API is described in [local-server setup](../../../development/local-server.md).

The broker is authenticated, has no upstream bridge, and uses nonpersistent sessions. Manual feed commands have durable caller UUIDs, QoS 0, and no retained messages. Replies must correlate to the request ID. An uncertain operation is not automatically republished. Stock firmware's limited raw-payload/result caching is not a substitute for durable no-replay handling.

## Feeding and device features

Recovered feeding code tracks feedback events, logical direction, single/double-bowl progress, direction pauses, and reverse recovery. The replacement controller uses bounded deadlines and fault stops; its policy is not a byte-for-byte reimplementation of every stock branch. Physical quantities and mechanical mapping remain uncalibrated.

Food/outlet optical filtering, power-dependent emitter duty, current estimates, battery lookup logic, four button roles, indicator selection, and audio completion paths have been investigated. A recovered branch does not establish physical signal polarity, actual timing, or complete feature support in replacement firmware.

## Panel buttons and child lock

Static analysis and offline emulation of the stock 1.1.15 panel handlers (October 18, 2026; nothing run on the feeder). Stock's event task runs its button handler once per loop with a 50-tick delay at 1 kHz, and every boundary below is a count of those visits, so the durations are nominal: handler and scheduling time add to them, and none was measured on the feeder.

| Behavior | Stock boundary | Nominal |
| --- | --- | --- |
| Lock (PB7) held toggles the panel lock, once per hold | 100th pressed visit | About 5 s |
| An unlocked panel relocks | 600th idle visit | About 30 s |
| Reset (PB4) held on an unlocked panel starts the factory restore | 100th pressed visit | About 5 s |
| Dispense (PB6) asks for one feed, once per press | The pressed visit | — |

- **Locked:** Dispense, Mic (PB5) and the Reset hold do nothing. Lock still toggles, so the panel can always be unlocked.
- **Idle:** activity on Mic, Reset or Dispense restarts the relock count.
- **Indicator:** PA4 is driven HIGH while locked and LOW while unlocked. The `lights` setting off drives every indicator LOW.
- **App setting:** the PLAF103 has no `childLock*` fields; the vendor app reports `childLockSwitch` and both lock durations as `null` for it. Its app-side setting is `disableHardwareButton` (configuration `+0x91`), which disables the panel except for a recovery sequence: Lock held about 3 s, then Reset held about 3 s, starts the factory restore.
- **Unknown:** the lock state at boot. Its flag's address falls inside the RAM image's code, so its initial value was not recovered.

**Replacement (FP11, physical on October 23 with the owner pressing; host-tested):** `petlibre-feeder`'s `panel` keeps these rules over the SDK button service's gestures, with one 5 s hold threshold, and the `feeder` firmware shows the lock on PA4. Choices where stock is unknown or differs:
- The panel starts locked at every boot, so an unattended reboot never leaves Dispense live.
- Any gesture, Lock included, restarts the relock, and a button still held holds it off until its release. Stock counts only Mic, Reset and Dispense.
- A signed `lock` request locks or unlocks from the network ([local transport](../../../firmware/local-transport.md#child-lock-fp11)). A remote unlock relocks on the same 30 s timer.
- `disableHardwareButton` and its recovery sequence are not implemented; FP15 owns resets.

## Bluetooth LE

Static analysis of the 1.1.15 KM4 image and the vendor app shows that stock links the full Realtek BLE stack, with its embedded controller patch and Wi-Fi coexistence. It runs as a GATT peripheral for two jobs only:
- **Setup:** the app writes Wi-Fi and MQTT settings and binds the device, then reads the result. It uses a 16-bit service `ffee` with characteristics `ffe1`–`ffe4`.
- **Offline diagnostics:** after about two minutes offline the feeder advertises and sends a compact report by notification, then stops advertising once it is back online.

Advertising runs only on mains power. Nothing found ties BLE to feeding, schedules or settings commands, which go over MQTT. Pairing appears only as SDK plumbing; the setup flow seems to rely on plain GATT with app-side encryption. Bond storage uses the SDK's FTL flash layer, whose sector was not located. The replacement's own BLE design is in [Bluetooth](../../../firmware/bluetooth.md).

## Clock and schedules

The server responds with the captured authoritative plan snapshot; it does not expose schedule editing or automatically push plans. Nonempty plans remain unvalidated for this release, and clock calibration is held when plans exist. Responses use the configured IANA timezone, including DST transition information. The full stock timezone/timer activation and cloud plan-construction contracts remain unresolved.

Replacement schedules still need durable occurrence identities, timezone/DST policy, restart reconciliation, and explicit handling of interrupted meals. A successful manual feed does not establish scheduled or unattended reliability.

## Feature limits and defaults (FP01)

Static pass over the 1.1.15 image and the vendor app (October 24, 2026), with the earlier unicorn emulations of the plan, manual-feed, panel, audio and Wi-Fi handlers. Labels: **static** (static analysis), **emulation** (instruction-level emulation of stock code, no hardware), **observed** (values the vendor app logged for the research unit; observed on one unit, not proven factory defaults). Durations counted in loop visits or timer ticks are nominal. Firmware defaults are what stock writes when its settings sector is blank or invalid.

| Card | Fact | Value | Evidence |
| --- | --- | --- | --- |
| FP02 | Stock control surface | No local control API; settings and actions arrive as MQTT attribute and service messages from the cloud | static |
| FP03 | Stored plans | At most 10; an 11th is dropped. Each stored entry is 44 bytes | emulation |
| FP03 | App plan limit | The app's copy says 10 schedules per day | static (app) |
| FP03 | Field validation | None: hour, minute, portions, `audioTimes` and weekday bytes store unchecked (256 wraps to 0, -1 to 255; "99:99" stores as given) | emulation |
| FP03 | Due check | Only second 0 of the plan's minute, on a listed weekday; a meal whose minute passes while the feeder is off is not caught up | emulation |
| FP03 | Busy and restart | A plan due during a feed waits and runs afterwards; a waiting plan survives restart and runs late with no age limit | emulation |
| FP03 | Time zone | The offset is stored as whole hours (5.5 becomes 5), so half-hour zones shift meals by 30 min | emulation |
| FP04 | Stored plan layout | Count at flash 0x7d0000, entries at 0x7d1000; non-repeating plans are deleted after they run | emulation, static |
| FP05 | Remote portions (`grainNum`) | One unchecked byte, 0–255; 256 wraps to 0; a request without `grainNum` gets ack 2000 | emulation |
| FP05 | Dispense button | One portion per press. A press during a feed starts nothing and is logged (record code 2022, or 2023 when the outlet reads blocked) | static, emulation |
| FP05 | Feed source in `GRAIN_OUTPUT_EVENT` | 1 schedule, 2 remote, 3 button | static, emulation (remote) |
| FP05 | App portion cap | Taken from a per-device cloud field, falling back to 24 when absent; the cloud sent `maxFeedingCup` 48 | static (app); observed |
| FP05 | Portion size | Unknown: no mass per portion is recorded. Settle by weighing a counted run of portions | — |
| FP06 | On-device records | Two flash rings (feeds, errors) of 51 slots each, 50 usable, 78-byte records. A feed record is deleted once the cloud acks its `GRAIN_END` with code 0, so the device keeps an upload queue, not a history | static |
| FP07 | `surplusGrain` | Boolean from the food beam: set as soon as the input reads low, cleared after 20 consecutive high readings. Physical polarity not verified | static |
| FP07 | Empty hopper | No food check in the schedule or button feed paths; the panel shows a food warning LED | static, emulation |
| FP08 | Motor jam | Motor current over 300 (ADC counts) reverses the motor; after the third reversal the feed ends with record code 2031 and counts as done (no retry) | static |
| FP08 | Outlet blocked (`grainOutletState` false) | The feed pauses; it resumes if the outlet clears within 600 one-second ticks (about 10 min), else it ends with record code 2008 | static |
| FP08 | Other record codes | 2003 a plan came due while busy; 2022 and 2023 Dispense pressed while busy | static |
| FP08 | `barnDoorError` | Not a firmware field; the cloud supplies it | static |
| FP09 | Desiccant | No firmware support; the cloud keeps the period and the count, and resets it through a cloud endpoint. `changeDesiccantFrequency` 14, `remainingDesiccantDays` 14 and -52 (keeps counting below zero). App text recommends 14 days | static; observed |
| FP09 | Period range | Unknown: the app's cycle picker bounds were not recovered | — |
| FP10 | Meal reminder | No firmware support; `feedingPlanAdvanceNoticeTime` 5 or null. Its unit and allowed values are unknown | static; observed |
| FP11 | Child lock | 5 s Lock hold toggles, 30 s idle relock (see [panel](#panel-buttons-and-child-lock)) | static, emulation |
| FP12 | Defaults | `volume` 50, `lightSwitch` and `soundSwitch` on, both aging types 1 (always on), windows 00:00–00:00, `disableHardwareButton` off; the app logged the same values | static; observed |
| FP12 | `volume` | One unchecked byte; 0 silences every sound. App slider 0–100. The mapping to codec gain is unknown | static; static (app) |
| FP12 | Quiet windows | Aging type 1 is always on, type 2 only inside start–end inclusive (a window may wrap midnight), any other value off. Compared with the device's local clock, although the fields are named `…Utc`; whether the app sends local or UTC times is unknown. `lightingTimes` and `soundTimes` are stored but not read by the window checks | static |
| FP12 | Lights off | Every indicator LOW | emulation |
| FP13 | Meal call clip | None until the app sets `enableAudio`: a URL naming the built-in call selects it, any other URL up to 256 bytes is downloaded (at most 321,024 bytes) | static |
| FP13 | When it plays | Before a scheduled feed only if the plan enables audio, a clip is set and sound is allowed (switch, window, volume above 0); it repeats `audioTimes` times, then the motor runs. A non-scheduled feed plays it twice under the same conditions. With sound blocked the feed starts at once | static, emulation |
| FP14 | `powerType` | 1 mains with the battery reading 0 % (or absent), 3 mains with battery charge, 2 battery only. The research unit reported 1 with `electricQuantity` 0 | static; observed |
| FP14 | Battery curve | ADC 1650 → 0 %, then 10 % per 60 counts up to 2250 → 100 %; a reading reports the first step whose threshold is at or above it | static |
| FP14 | Low battery | Warning LED at 20 % or less whenever a battery reads (on battery or on mains); none on mains with no battery reading. Below ADC 1650 on battery, stock stops motors, emitters and LEDs and polls for mains every second, restarting when it returns | emulation; static |
| FP14 | Battery mode radios | A configured feeder shuts Wi-Fi and MQTT 5 s after switching to battery; an unconfigured one closes its AP and BLE. With no clock sync on battery it gives up joining after 60 s; with sync it does not retry | static, emulation |
| FP14 | Battery mode schedules | The meal dispatch has no power-type check, so meals run on battery from the running clock. Beam emitters are duty-cycled (60 visits on, 200 off) | static |
| FP14 | OTA | Refused on battery only (code 1110) and during a feed (1120) | static |
| FP14 | `enableBatteryPercent`, `autoChangeMode`, `autoThreshold` | The first is cloud-only (`false` observed). The others are firmware settings (defaults 1 and 20) whose effect is unknown | static; observed |
| FP15 | Reset holds | One hold: Reset about 5 s on an unlocked panel starts the factory restore. No Wi-Fi-only hold exists in the panel handler | static, emulation |
| FP15 | What the restore clears | Unknown: its consumer task was not decompiled. Settle by decompiling it and listing the flash sectors it erases | — |
| FP16–FP19 | Stock counterpart | None on the device: no Home Assistant, history and remote control live in the cloud, and the vendor app is the client | static |

**App and cloud, not firmware:** the desiccant reminder, the meal reminder, the low-battery and other notifications (`enableLowBatteryNotice` and siblings), `enableBatteryPercent`, `barnDoorError`, feed history (the device only queues unsent records), the per-model portion cap and the plan editor's limits. The firmware has no strings or fields for any of these.

## Reliability limits

A stock-only server-restart experiment reproduced repeated startups and a KM4 watchdog reset indication. The exact stall trigger is unresolved. WAN isolation, sustained reconnect, cold boot, time recovery, battery behavior, and portion weight require independent physical acceptance. Home Assistant integration remains deferred.

# Firmware feature board

This board tracks replacement-firmware integration, dependencies, and acceptance criteria. Status records evidence from the research unit only; it is not a compatibility guarantee for another unit. For the installed artifact, bench state, evidence index and next task, start with [RESUME.md](RESUME.md).

## Current status

- **Done:** FW01–FW06 and FW15, five of them with recorded scope limits (see carried items).
- **In progress:** FW07 (stage B passed: radio on and scanning on hardware, with fast connect off and a link audit pinning every flash/eFuse write reference; stage C passed: joins, gets a DHCP lease, and rejoins after the access point returns, with KM0 now running the WLAN firmware; DNS and egress untested). FW14 waits for batteries: its ADC channel select and millivolt scaling are proven, but the battery channel, PA19 and the divider are not.
- **Critical path to feeding:** FW13/FW14/FW15 → FW30 → FW16 → FW17.

### Carried items

Work a Done card deliberately handed to a later card. Close a row only with the evidence its owner card names.

| Item | From | Owner | Notes |
| --- | --- | --- | --- |
| Motor STOP and infrared-off voltages, startup transients, repeated measured resets | FW03 | FW30 | Software readback and observed no-motion only |
| Port A indicators PA0/PA2/PA4 output-only | FW04 | FW23 | No input readback on the tested unit; levels rest on visual observation |
| Production watchdog clock bounds and counter choice | FW05 | FW16 | Effective clock measured near 43 kHz (FW05) and near 31–33 kHz (FW06 stage C) with the same counter; the cause is unknown. Bounds widened to 24–54 kHz; explain the spread before FW16 relies on it |
| Owner-lease expiry while a task stalls, on hardware | FW05 | FW16 | Host-tested; FW05 proved the supervision timer keeps running during a stall |
| Stop timing under supervised motion | FW05 | FW19 | FW05 stall tests ran with motion disabled |
| KM0 SDK flash users (DLPS tickless lock path) unmediated; KM0 deny provider | FW06 | FW26 | Unreachable while KM0 holds its wakelock; mediate or deny before KM0 may sleep |
| Exclusion for SDK flash users not yet exercised (Wi-Fi, OTA, FTL) | FW06 | FW07 | FW06 proved the hooked read, sector erase and page program paths only |
| Journal reservation before real motion and result after verified stop | FW15 | FW16 | Journal reserve, finish, pending refusal and torn-record recovery passed on NOR with no motion; the owner wiring around real motion belongs to FW16 |

## Using the board

Move cards through **Planned → Ready → In progress → Bench check → Done**. Ready requires completed dependencies. A Done card that leaves part of its finish line to another card says so in its status and in the carried-items table. The Evidence column records what acceptance rests on: physical bench observation, static analysis, or host simulation. Update the board when meaningful implementation or acceptance evidence changes. Keep one integration card in progress; independent software preparation can proceed in parallel.

Hardware acceptance belongs to an explicitly selected bench device and an operator-supervised procedure. No status here authorizes flashing, motion, or another stock round trip. Preserve raw evidence privately and record redacted outcomes. Host tests alone do not complete hardware acceptance.

Prioritize the UART development loop and build identity, stopped startup, LED diagnostics, then real flash coordination and Wi-Fi/OTA. Sensors and durable storage lead into supervised feeding; everyday features and fault/sustained testing follow. See [architecture](docs/firmware/README.md), [startup/indicator diagnostics](docs/firmware/startup-diagnostics.md), [development updater](docs/development/uart-development.md), and [recovery limits](docs/hardware/recovery.md).

## Faster development and initial hardware checks

| Card | Status | Evidence | Prerequisites | Finish line |
| --- | --- | --- | --- | --- |
| **FW01 Faster UART application updates** | **Done** | Physical | Completed recovery baseline | Accept a reviewed new build, write the required application sectors and verify that region. Preserve factory/settings/audio and validate bank bounds, both core images, marker order and the current source image. Stop on unexpected drift or protocol failure. Demonstrate changed-build update and boot on the research unit and record actual duration. Retain the demonstrated conservative stock-recovery path; a separate stock return through the new updater is deferred by the recorded research scope. |
| **FW02 Build identity and repeatable startup reports** | **Done** | Physical | FW01 | Each build reports its identity, active bank, boot/reset reason and diagnostic result. The host can match startup to the intended artifact and distinguish success, timeout and repeated boot. No credentials in logs. |
| **FW03 Stopped startup and GPIO readback** | **Done (revised scope)** | Physical (readback, observation) | FW01, FW02 | Establish stock-derived STOP/off startup, exact GPIO values/directions, artifact-bound normal boot and operator movement observation while the backend stays unopened. The recorded scope defers independent voltage, repeated measured reset and transient checks to FW30; these remain unverified. |
| **FW04 First LED controller** | **Done (output-only scope)** | Physical (capture, observation) | FW03 | Identify LED pins, colors and polarity physically. Exercise off/on/blink from a small diagnostic, with bounded timing and no motor or infrared enable. This is the first visible feature experiment. |
| **FW05 Clock, independent stop timer and watchdog** | **Done (motion-disabled scope)** | Physical (UART timing) | FW03 | With motion disabled: measure the SYSTIMER rate against host time and confirm it only moves forward; show the supervision timer interrupt keeps running while a task stalls; show the watchdog refreshes only on real progress and resets the platform when progress stops; show a deliberate reset cannot become a reset loop. Lease expiry under a stall on hardware, production watchdog bounds and stop timing under motion are carried to FW16, FW06 and FW19. |
| **FW31 Bench automation** | Ready | — | FW01 | Software-controlled feeder power and loader entry (for example a USB relay, and the adapter's spare control lines driving the loader button through a transistor), so the host can flash, power-cycle and capture without manual steps. Never powers the motor path while flashing; the operator can always cut power by hand. Record the wiring and its failure modes. |
| **FW06 Real flash coordination across both cores** | **Done (mediated-path scope)** | Physical (UART protocol and timing) | FW03, FW05 | Implement the missing transport/provider and fresh boot handshake, RAM parking and mediated SDK flash operations. Verify stopped outputs before granting writes, bounded failure handling and no access from the other core during a lease. Inject peer loss and timeout. Fix the production watchdog configuration from the FW05 measurement, with margin for clock spread, and size the lease budget against it. |

## Early Wi-Fi and OTA

| Card | Status | Evidence | Prerequisites | Finish line |
| --- | --- | --- | --- | --- |
| **FW07 Wi-Fi join and reconnect** | In progress | Physical (UART + hotspot DHCP log: scans, join, lease, loss and rejoin) | FW03, FW06 | Join the configured test network, obtain an address and recover from AP/DHCP/DNS loss without repeated uncontrolled resets. Define credential storage and audit SDK writes through the flash authority. Bringup credentials stay private; the full setup flow is FW25. |
| **FW08 Local health and control transport** | Planned | — | FW02, FW07 | Expose build/bank/boot identity, uptime and fault state through a versioned local interface. Define authentication and compatibility with the existing Rust server. Health requests and diagnostic commands cannot dispense food. |
| **FW09 OTA bank selection and boot policy** | Ready | — | FW01, FW06 | Establish usable bank capacities, selection with two valid banks, image compatibility and activation rules. Define a failed-first-boot recovery mechanism that the actual factory loader can support. Document any limitation before relying on rollback; do not infer it from the presence of two banks. |
| **FW10 Guarded OTA upload and activation** | Planned | — | FW03, FW06, FW07, FW08, FW09 | Authenticate updates, validate the manifest/image and size, refuse the running bank and preserve unrelated data. Verify the candidate before activation and reboot into the expected build. Test corrupt, truncated and interrupted transfers with a recoverable boot path. |
| **FW11 Boot confirmation and failed update recovery** | Planned | — | FW09, FW10 | Implement and test the chosen trial/confirmation policy. Exercise a valid image that fails to start or confirm, and a lost confirmation response. Establish recovery on hardware; clearly distinguish automatic rollback from a path requiring UART. |
| **FW12 Automated development loop** | Planned | — | FW02, FW08, FW10, FW11 | Script build → checks → upload → reboot → verify build identity and health. Retain artifacts/results, detect loops/timeouts and stop on uncertain updates. Demonstrate consecutive changed builds without manual power cycles on the successful path; record duration and recovery limits. |

## Sensors and local feeding

| Card | Status | Evidence | Prerequisites | Finish line |
| --- | --- | --- | --- | --- |
| **FW13 Food, outlet and feedback inputs** | Ready | — | FW03, FW05, FW06 | Confirm emitter/receiver pairs and blocked/clear polarity. Measure settling, pulse capture and bounded emitter duty. Reject stale/invalid inputs; observe owner stalls with emitters active and motor disabled. |
| **FW14 Current, battery and external power inputs** | In progress | Physical (UART readback, internal supply channel, no batteries) | FW03, FW05 | Enable the LP ADC clock explicitly, then verify ADC readiness/calibration, voltage scaling and external-power detection against measurements. Establish sample freshness and report unknown values honestly. Calibrate loaded motor current during FW16/FW19. |
| **FW15 Durable settings and feeding journal** | **Done (storage scope)** | Physical (UART protocol, NOR power cuts) | FW06 | Establish explicit storage allocation, implement real NOR I/O and schema/version recovery, and bound wear. Persist reservation before motion and result after verified stop. Test interrupted writes and block ambiguous pending feeds after restart. |
| **FW30 Electrical output verification** | Ready | — | FW03 | Before any intended motion, measure motor driver inputs at STOP, infrared emitter OFF, and startup/reset transients across repeated measured power-ons and resets (multimeter; scope or logic analyzer for transients). Confirm stock-derived polarity electrically. |
| **FW16 Supervised motor controller and feeding owner** | Planned | — | FW05, FW06, FW13, FW14, FW15, FW30 | Wire the existing owner/controller to real ports. Show owner-lease expiry under a stalled owner with motion disabled, then verify a bounded pulse, direction, feedback and STOP under supervision before a portion test. Exercise cancellation, lease expiry and failed stop; a failed reservation cannot start motion. |
| **FW17 Local manual feeding and durable results** | Planned | — | FW08, FW15, FW16 | Validate command IDs, quantities and bowl modes; serialize requests and prevent duplicate/replayed delivery across reboot. Distinguish accepted, durably reserved, completed and uncertain results. Local status remains usable during a feed. |
| **FW18 Portion and bowl calibration** | Planned | — | FW16, FW17 | Measure delivered food separately from requested counts. Establish supported single/double-bowl direction and phase behavior. An unknown phase blocks unsafe requests; calibration parameters persist with documented limits. |
| **FW19 Jams, missing feedback and sensor faults** | Planned | — | FW05, FW13, FW14, FW16 | Verify independent stop while the owner stalls, bounded reverse attempts, total runtime and current limits. Test blocked outlet, missing/stuck sensors and ADC failure; end stopped with an explicit result and no automatic restart of an uncertain feed. |

## Everyday firmware features

| Card | Status | Evidence | Prerequisites | Finish line |
| --- | --- | --- | --- | --- |
| **FW20 Buttons, panel lock and reset behavior** | Planned | — | FW03, FW15, FW17 | Match physical labels and levels; implement debounce, holds, simultaneous inputs and relock. Feeding buttons use the same owner as network requests. Define exactly which settings a reset clears and preserve recovery access. |
| **FW21 Clock, timezone and missed time policy** | Planned | — | FW07, FW15 | Track clock validity, RTC/NTP sync and timezone/DST. Define forward/backward jump and missed-meal rules before scheduling; no schedule runs from an invalid clock. Verify selected rules across restart and time changes. |
| **FW22 Persistent local schedules** | Planned | — | FW15, FW17, FW18, FW21 | Create/edit/disable schedules locally and execute each occurrence once through the feeding owner. Cover DST, clock jumps, overlaps, restart and interrupted-meal reconciliation. Internet access is unnecessary for already valid local schedules. |
| **FW23 Complete indicator behavior** | Planned | — | FW04, FW08, FW19, FW20, FW21 | Show connection, lock, update and fault states with explicit priority and cancellation. Implement light disable and quiet windows; verify colors/polarities and state transitions on the board. |
| **FW24 Buzzer and audio playback** | Planned | — | FW03, FW15, FW17, FW21 | Establish supported assets/output, volume and sound settings. Bound playback and queueing independently of feeding. Muted, missing, busy or failed audio must not prevent a motor stop or a terminal feed result. |
| **FW25 Local setup and configuration** | Planned | — | FW07, FW15, FW20 | Provide a documented local way to set Wi-Fi, credentials and device options without the vendor cloud. Validate inputs, keep secrets out of telemetry, survive interrupted saves and recover after bad credentials or an intentional settings reset. |
| **FW26 Battery transitions, brownout and sleep** | Planned | — | FW05, FW14, FW15, FW16 | Verify battery/external-power changes and low-voltage policy. Sleep only with verified inactive outputs; handle clock/watchdog ownership and wake without unintended motion or replay. Report uncertain voltage/charge state explicitly. |

## Reliability and integration

| Card | Status | Evidence | Prerequisites | Finish line |
| --- | --- | --- | --- | --- |
| **FW27 Combined fault and sustained-operation tests** | Planned | — | FW11, FW18, FW19, FW20, FW22, FW23, FW24, FW25, FW26 | Run recorded scenarios for network loss/replay, task stalls, sensor faults, jams, low power and supervised power cuts during journal/OTA/feeding phases. Verify recovery, inactive outputs and explicit uncertainty. Define and complete a sustained-run duration and success criteria before the run. |
| **FW28 Home Assistant integration** | Planned | — | FW17, FW18, FW19, FW22, FW25 | After useful standalone local behavior is established, add discovery, availability, sensors, settings and feed commands using the documented local interface. Reconnect or automation replay cannot duplicate a feed. Core feeding continues without Home Assistant. |
| **FW29 Research firmware release** | Planned | — | FW27 | Freeze a build and compatibility manifest, record supported features and measured limits, and publish repository installation/update/recovery instructions. State unresolved risks and preserve a tested recovery path. Deployment beyond the tested research unit needs separate hardware acceptance. |

## Status at a glance

✅ Done · 🚧 In progress · 🧪 Bench check · 🟢 Ready · ⬜ Planned. The cards above are authoritative.

| Card | Status |
| --- | --- |
| FW01 Faster UART application updates | ✅ Done |
| FW02 Build identity and repeatable startup reports | ✅ Done |
| FW03 Stopped startup and GPIO readback | ✅ Done (revised scope) |
| FW04 First LED controller | ✅ Done (output-only scope) |
| FW05 Clock, independent stop timer and watchdog | ✅ Done (motion-disabled scope) |
| FW06 Real flash coordination across both cores | ✅ Done (mediated-path scope) |
| FW07 Wi-Fi join and reconnect | 🚧 In progress |
| FW08 Local health and control transport | ⬜ Planned |
| FW09 OTA bank selection and boot policy | 🟢 Ready |
| FW10 Guarded OTA upload and activation | ⬜ Planned |
| FW11 Boot confirmation and failed update recovery | ⬜ Planned |
| FW12 Automated development loop | ⬜ Planned |
| FW13 Food, outlet and feedback inputs | 🟢 Ready |
| FW14 Current, battery and external power inputs | 🚧 In progress |
| FW15 Durable settings and feeding journal | ✅ Done (storage scope) |
| FW16 Supervised motor controller and feeding owner | ⬜ Planned |
| FW17 Local manual feeding and durable results | ⬜ Planned |
| FW18 Portion and bowl calibration | ⬜ Planned |
| FW19 Jams, missing feedback and sensor faults | ⬜ Planned |
| FW20 Buttons, panel lock and reset behavior | ⬜ Planned |
| FW21 Clock, timezone and missed time policy | ⬜ Planned |
| FW22 Persistent local schedules | ⬜ Planned |
| FW23 Complete indicator behavior | ⬜ Planned |
| FW24 Buzzer and audio playback | ⬜ Planned |
| FW25 Local setup and configuration | ⬜ Planned |
| FW26 Battery transitions, brownout and sleep | ⬜ Planned |
| FW27 Combined fault and sustained-operation tests | ⬜ Planned |
| FW28 Home Assistant integration | ⬜ Planned |
| FW29 Research firmware release | ⬜ Planned |
| FW30 Electrical output verification | 🟢 Ready |
| FW31 Bench automation | 🟢 Ready |

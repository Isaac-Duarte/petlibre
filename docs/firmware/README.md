# Firmware architecture and status

## Two independent implementations

The local server talks to **stock 1.1.15** over its existing MQTT protocol. It does not run inside the feeder. The replacement firmware uses Rust application components on the Realtek AmebaD SDK. On hardware it has stopped-output startup, Wi-Fi and an encrypted device API, over-the-air updates with trial rollback, durable settings, buttons and the child lock, power inputs, audio and BLE setup ([firmware board](../../FIRMWARE_BOARD.md)); functional feeding remains unimplemented on hardware.

Stock 1.1.15 can install the replacement over its own updater, with no UART write (physical, FW40). [Consumer installation](consumer-installation.md) covers stock's setup protocol, the install flow an app would run, the way back to stock, and the server and app architecture. [Stock OTA requirements](../devices/plaf103/stock/ota-requirements.md) has the image contract. [Product architecture](product-architecture.md) has the target the SDK is built toward: one firmware, an optional server and a feeder-hosted API. [Resource budget](resource-budget.md) measures whether that product fits the PLAF103 (FW44). [Scheduling gaps](scheduling-gaps.md) is the `feeder` firmware's `gap-probe` variant, which measures how long the loop and a priority-4 task wait across Wi-Fi rejoins. [Device layer](device-layer.md) lists what is specific to the PLAF103 and where it lives (FW50).

| Crate | Responsibility |
| --- | --- |
| `wellfed-session` | `no_std` session layer the feeder and its clients share: Noise handshakes and transport, HMAC, the setup code, length-prefixed frames, and strict hex and base64 |
| `wellfed-core` | `no_std` controller, portion accounting, sensor qualification, ADC scaling, motor gate, feedback queue, monotonic clock, flash authority, and the SDK's stores (journal, seed, update store) over the shared record layer |
| `wellfed-hal-common` | The routines both cores link: RAM parking, SDK flash hooks, the KM0 park peer, and the OTA boot-trial step. The KM0 image contains only this crate |
| `wellfed-hal` | KM4 ports: GPIO/ADC board port, interrupt/timer supervision, watchdog, FreeRTOS owner, flash lease provider, transport, updater, build-bound startup reports, stopped outputs, and LED diagnostics. It re-exports the common crate's `flash` and `trial` |
| `wellfed-sdk` | The KM4 runtime every firmware runs inside: the `wellfed_app_start` entry, stopped-output startup and the synthetic diagnostic, the Wi-Fi join and health transport, the watchdog feeder, FW11 confirmation, the settings writer, and the FW10 updater and offer check. Bank 0 runs only the runtime. A firmware defines its start hook with `firmware!` and creates its tasks with `spawn` |
| `wellfed-probes` | The bench probes, once for every firmware that runs them: optics (FW13), buttons (FW20), power inputs (FW14), the motor pulse (FW16), the speaker records, and the retired flash-stress and supervision probes, kept for their host tests |
| `wellfed-proto` (`src/product/wellfed-proto`) | The device protocol's messages, `no_std` and allocation-free, shared by the feeder and its clients |
| `wellfed-feeder` (`src/product/wellfed-feeder`) | Product policy, `no_std` and host-tested: the panel's child lock (FP11), pairing and BLE setup (FW45), the [device API](device-api.md)'s sans-IO server (FW47), and the product's stores (settings, Wi-Fi credentials, client keys and the event ring) over core's record layer; schedules and alerts later ([product architecture](product-architecture.md)) |
| `wellfed-fw-feeder` (`src/firmware/wellfed/feeder`) | The product firmware: the button service and one loop task that owns the panel, BLE setup mode and the [device API](device-api.md) with its sockets and mDNS, and steps the feeding owner (idle at STOP until feed requests exist) |
| `wellfed-fw-qa` (`src/firmware/wellfed/qa`) | The default firmware: a [guided self-test](qa.md) of optics, power, buttons, indicators and speaker |
| `wellfed-fw-optics`, `-buttons`, `-adc`, `-speaker`, `-motor`, `-trial` | One bring-up area each ([table](../../src/firmware/wellfed/README.md)); `motor` drives the motor only in its `pulse` variant |
| `wellfed-fw-blink` (`src/firmware/wellfed/blink`) | The minimal firmware: one task blinks the connection indicator. The [firmware guide](../../src/firmware/wellfed/README.md) covers writing one |
| `wellfed-amebad-sys` | Bindgen-backed KM4 calls into the pinned AmebaD SDK, and the deterministic host fake SDK used by tests and the simulator |
| `wellfed-board-plaf103` | The PLAF103 board: every pin and ADC channel the ports use, the storage region and its sector map |
| `wellfed-sdk-c` | The KM0 archive, and the panic handler every firmware archive links |
| `wellfed-testkit`, `wellfed-sim` | Fake NOR journal storage and deterministic host worker |

Minimal C hooks call the Rust entrypoint. The SDK still supplies startup, FreeRTOS, radio code, linker scripts, and image packaging. KM0 and KM4 are built separately. Header-derived bindings are generated for SDK-backed KM4 builds; host tests use fake bindings.

## Runtime contracts

The owner serializes requests and drives the controller from qualified inputs and captured feedback. The output gate uses stop intervals, leases, and fault latches. Timer supervision can stop drive when the owner stalls; one timer interrupt without SYSTIMER advance is tolerated (a pending update coalesced after a masked window), a second consecutive one is a frozen-clock fault. Closing supervision first verifies motor STOP and infrared OFF. Infrared OFF always requires LOW output latches, not only LOW pad inputs. Watchdog refresh requires progress, and the owner refuses a flash authority whose total lease budget plus two sample and two timer periods does not fit inside the watchdog progress gap. Interrupted feeding is reconciled conservatively rather than automatically resumed: at the next boot a pending reservation is recorded in the journal as an uncertain result under its own id, reported once as such and never retried, and the owner opens. A journal that does not decode, a resolution that cannot be recorded, or another initialization failure after the watchdog is armed latches a stopped fault, which keeps refreshing only while every iteration re-verifies STOP and infrared OFF, so it neither retries nor reset-loops, while a hang or lost output readback still resets. On KM4 a panic masks interrupts and requests motor STOP and infrared OFF with raw latch writes before halting; only an already armed watchdog resets it.

The journal uses alternating complete records, CRC, and a final commit marker. Host tests exercise torn writes and restart behavior against a fake NOR at 256-byte, 4 KiB and odd-tail unit sizes. The fake models erase to 0xFF, bit-clearing programs, byte-ordered program cuts, and erase cuts that leave old data with an arbitrary deterministic subset of bits raised. It does not model weakly programmed or erased cells, unstable reads, page wrap, or a real part's timing; this is simulation, not flash evidence. A working physical flash adapter and verified storage allocation are still missing. Flash coordination, SDK hooks, and RAM parking are tested components; complete exclusive flash ownership across both cores is unfinished.

The core diagnostic uses synthetic inputs and memory-only motor outputs. Separately, the default application explicitly enables GPIO clocks, requests physical motor STOP/infrared-off, and runs a bounded LED probe. The minimal KM0 main restores the stock shared-pad supply prerequisite. The full feeding backend remains unopened. Voltage/transient acceptance and Port A input-readback faults remain unresolved; see [startup diagnostics](startup-diagnostics.md).

## Capability and acceptance map

| Area | Present | Hardware/integration work remaining |
| --- | --- | --- |
| Manual feeding and single/double bowls | Host-tested controller, direction/portion accounting, bounded recovery | Mechanical phase, bowl mapping, measured portions, jams, verified stops |
| Inputs and power | Host-tested filtering, stale/invalid-input rejection, ADC scaling | Calibration, optical timing/polarity, current limits, battery/brownout behavior |
| Request persistence | Host-tested journal and owner integration | Physical flash provider, durable writes, power cuts |
| Runtime reliability | Host-tested leases, ISR supervision, watchdog, and flash coordination | Real timing, reset/sleep behavior, sustained runs and complete flash mediation |
| Networking and local protocol | Stock server implemented separately; replacement Wi-Fi join, DHCP and reconnect on hardware (FW07), with link liveness from association, lease and gateway rather than internet DNS (host-tested, October 25); the Noise [device API](device-api.md) over TCP with mDNS, events and samples (FW47), stored Wi-Fi credentials and device settings (FW25), and BLE setup (FW45), all on hardware; updates over the device API with bank 0 running the feeder (host-tested, October 25; no build opens an MQTT session) | The first bank-0 install and round trip on hardware; deleting the MQTT transport and its credentials (AR6); recovery over the LAN on hardware |
| Schedules | Stock semantics investigated | Replacement persistence, timezone/DST, occurrence identity, interrupted-meal policy |
| Panel and audio | Stock roles partially recovered; [button service](buttons.md) (debounce, holds, chords) and audio playback on hardware | Lock and reset behavior (FP11, FP15), complete indicator policies, audio deadlines |
| Home Assistant | Paused by the owner (Oct 23); two [options](product-architecture.md#home-assistant) recorded (FW49, FP16) | Everything; follows the device-hosted API (FW47) |

Rust ports retain caller-owned objects shared with ISRs through critical sections; a fully interrupt-safe ownership model remains future work. SDK build success and model coverage do not establish electrical safety or physical reliability.

Continue with [boot and flash](../devices/plaf103/flash-layout.md), [audio](audio.md), [buttons](buttons.md), [Bluetooth](bluetooth.md), [stock behavior](../devices/plaf103/stock/README.md), [SDK builds](../development/firmware-builds.md), or [host simulation](../development/README.md).

Use the [firmware feature board](../../FIRMWARE_BOARD.md) for FW01–FW29 prerequisites, current acceptance scope, and remaining work.

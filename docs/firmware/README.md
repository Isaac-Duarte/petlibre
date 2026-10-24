# Firmware architecture and status

## Two independent implementations

The local server talks to **stock 1.1.15** over its existing MQTT protocol. It does not run inside the feeder. The replacement firmware uses Rust application components on the Realtek AmebaD SDK. Its current physical milestones include build-bound diagnostic startup, stopped-output GPIO startup, and bounded visible indicator control; functional feeding remains unimplemented on hardware.

Stock 1.1.15 can install the replacement over its own updater, with no UART write (physical, FW40). [Consumer installation](consumer-installation.md) covers stock's setup protocol, the install flow an app would run, the way back to stock, and the server and app architecture. [Stock OTA requirements](../devices/plaf103/stock/ota-requirements.md) has the image contract. [Product architecture](product-architecture.md) has the target the SDK is built toward: one firmware, an optional server, a feeder-hosted API and native Home Assistant. [Resource budget](resource-budget.md) measures whether that product fits the PLAF103 (FW44). [Device layer](device-layer.md) lists what is specific to the PLAF103 and where it lives (FW50).

| Crate | Responsibility |
| --- | --- |
| `wellfed-core` | `no_std` controller, portion accounting, sensor qualification, ADC scaling, motor gate, feedback queue, monotonic clock, journal, and flash authority |
| `wellfed-hal-common` | The routines both cores link: RAM parking, SDK flash hooks, the KM0 park peer, and the OTA boot-trial step. The KM0 image contains only this crate |
| `wellfed-hal` | KM4 ports: GPIO/ADC board port, interrupt/timer supervision, watchdog, FreeRTOS owner, flash lease provider, transport, updater, build-bound startup reports, stopped outputs, and LED diagnostics. It re-exports the common crate's `flash` and `trial` |
| `wellfed-sdk` | The KM4 runtime every firmware runs inside: the `wellfed_app_start` entry, stopped-output startup and the synthetic diagnostic, the Wi-Fi join and health transport, the watchdog feeder, FW11 confirmation, the settings writer, and the FW10 updater and offer check. Bank 0 runs only the runtime. A firmware defines its start hook with `firmware!` and creates its tasks with `spawn` |
| `wellfed-probes` | The bench probes, once for every firmware that runs them: optics (FW13), buttons (FW20), power inputs (FW14), the motor pulse (FW16), the speaker records, and the retired flash-stress and supervision probes, kept for their host tests |
| `wellfed-fw-qa` (`src/firmware/wellfed/qa`) | The default firmware: a [guided self-test](qa.md) of optics, power, buttons, indicators and speaker |
| `wellfed-fw-optics`, `-buttons`, `-adc`, `-speaker`, `-motor`, `-trial` | One bring-up area each ([table](../../src/firmware/wellfed/README.md)); `motor` drives the motor only in its `fw16-pulse` variant |
| `wellfed-fw-blink` (`src/firmware/wellfed/blink`) | The minimal firmware: one task blinks the connection indicator. The [firmware guide](../../src/firmware/wellfed/README.md) covers writing one |
| `wellfed-amebad-sys` | Bindgen-backed KM4 calls into the pinned AmebaD SDK, and the deterministic host fake SDK used by tests and the simulator |
| `wellfed-sdk-c` | The core C ABI, the KM0 archive, and the panic handler every firmware archive links |
| `wellfed-testkit`, `wellfed-sim` | Fake NOR journal storage and deterministic host worker |

Minimal C hooks call the Rust entrypoint. The SDK still supplies startup, FreeRTOS, radio code, linker scripts, and image packaging. KM0 and KM4 are built separately. Header-derived bindings are generated for SDK-backed KM4 builds; host tests use fake bindings.

## Runtime contracts

The owner serializes requests and drives the controller from qualified inputs and captured feedback. The output gate uses stop intervals, leases, and fault latches. Timer supervision can stop drive when the owner stalls; one timer interrupt without SYSTIMER advance is tolerated (a pending update coalesced after a masked window), a second consecutive one is a frozen-clock fault. Closing supervision first verifies motor STOP and infrared OFF. Infrared OFF always requires LOW output latches, not only LOW pad inputs. Watchdog refresh requires progress, and the owner refuses a flash authority whose total lease budget plus two sample and two timer periods does not fit inside the watchdog progress gap. Interrupted feeding is reconciled conservatively rather than automatically resumed: a pending reservation or another initialization failure after the watchdog is armed latches a stopped fault, which keeps refreshing only while every iteration re-verifies STOP and infrared OFF, so it neither retries nor reset-loops, while a hang or lost output readback still resets. On KM4 a panic masks interrupts and requests motor STOP and infrared OFF with raw latch writes before halting; only an already armed watchdog resets it.

The journal uses alternating complete records, CRC, and a final commit marker. Host tests exercise torn writes and restart behavior against a fake NOR at 256-byte, 4 KiB and odd-tail unit sizes. The fake models erase to 0xFF, bit-clearing programs, byte-ordered program cuts, and erase cuts that leave old data with an arbitrary deterministic subset of bits raised. It does not model weakly programmed or erased cells, unstable reads, page wrap, or a real part's timing; this is simulation, not flash evidence. A working physical flash adapter and verified storage allocation are still missing. Flash coordination, SDK hooks, and RAM parking are tested components; complete exclusive flash ownership across both cores is unfinished.

The core diagnostic uses synthetic inputs and memory-only motor outputs. Separately, the default application explicitly enables GPIO clocks, requests physical motor STOP/infrared-off, and runs a bounded LED probe. The minimal KM0 main restores the stock shared-pad supply prerequisite. The full feeding backend remains unopened. Voltage/transient acceptance and Port A input-readback faults remain unresolved; see [startup diagnostics](startup-diagnostics.md).

## Capability and acceptance map

| Area | Present | Hardware/integration work remaining |
| --- | --- | --- |
| Manual feeding and single/double bowls | Host-tested controller, direction/portion accounting, bounded recovery | Mechanical phase, bowl mapping, measured portions, jams, verified stops |
| Inputs and power | Host-tested filtering, stale/invalid-input rejection, ADC scaling | Calibration, optical timing/polarity, current limits, battery/brownout behavior |
| Request persistence | Host-tested journal and owner integration | Physical flash provider, durable writes, power cuts |
| Runtime reliability | Host-tested leases, ISR supervision, watchdog, and flash coordination | Real timing, reset/sleep behavior, sustained runs and complete flash mediation |
| Networking and local protocol | Stock server implemented separately; replacement Wi-Fi join, DHCP, DNS checks and reconnect on hardware (FW07); health transport [designed](local-transport.md) (FW08) | Health/diagnostic transport, provisioning, commands, authenticated channel |
| Schedules | Stock semantics investigated | Replacement persistence, timezone/DST, occurrence identity, interrupted-meal policy |
| Panel and audio | Stock roles partially recovered; [button service](buttons.md) (debounce, holds, chords) and audio playback on hardware | Lock and reset behavior (FP11, FP15), complete indicator policies, audio deadlines |
| Home Assistant | [Designed](product-architecture.md#home-assistant): the ESPHome native API, auto-discovered (FW49, FP16) | Everything; follows the device-hosted API (FW47) |

Rust ports retain caller-owned objects shared with ISRs through critical sections; a fully interrupt-safe ownership model remains future work. SDK build success and model coverage do not establish electrical safety or physical reliability.

Continue with [boot and flash](../devices/plaf103/flash-layout.md), [audio](audio.md), [buttons](buttons.md), [Bluetooth](bluetooth.md), [stock behavior](../devices/plaf103/stock/README.md), [SDK builds](../development/firmware-builds.md), or [host simulation](../development/README.md).

Use the [firmware feature board](../../FIRMWARE_BOARD.md) for FW01–FW29 prerequisites, current acceptance scope, and remaining work.

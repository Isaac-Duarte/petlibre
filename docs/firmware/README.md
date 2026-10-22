# Firmware architecture and status

## Two independent implementations

The local server talks to **stock 1.1.15** over its existing MQTT protocol. It does not run inside the feeder. The replacement firmware uses Rust application components on the Realtek AmebaD SDK. Its current physical milestones include build-bound diagnostic startup, stopped-output GPIO startup, and bounded visible indicator control; functional feeding remains unimplemented on hardware.

| Crate | Responsibility |
| --- | --- |
| `wellfed-core` | `no_std` controller, portion accounting, sensor qualification, ADC scaling, motor gate, feedback queue, monotonic clock, journal, and flash authority |
| `wellfed-amebad-common` | The routines both cores link: RAM parking, SDK flash hooks, the KM0 park peer, and the OTA boot-trial step. The KM0 image contains only this crate |
| `wellfed-amebad` | KM4 ports: GPIO/ADC board port, interrupt/timer supervision, watchdog, FreeRTOS owner, flash lease provider, transport, updater, build-bound startup reports, stopped outputs, and LED diagnostics. It re-exports the common crate's `flash` and `trial` |
| `wellfed-firstboot` | The KM4 first-boot application entry and its bench probes (Wi-Fi, optics, ADC, supervision, flash stress, and the opt-in `fw16-pulse` motion variant) |
| `wellfed-amebad-sdk` | Bindgen-backed KM4 calls into the pinned AmebaD SDK, and the deterministic host fake SDK used by tests and the simulator |
| `wellfed-ffi` | C ABI and per-core static libraries joining Rust to SDK startup |
| `wellfed-testkit`, `wellfed-sim` | Fake NOR journal storage and deterministic host worker |
| `wellfed-probe` | Small non-actuating GET_CONFIG execution probe embedded into a fixed stock-image slot |

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
| Panel and audio | Stock roles partially recovered | Replacement buttons/lock/reset, complete indicator policies, audio playback and deadlines |
| Home Assistant | Deferred | Interface and integration after useful standalone operation |

Rust ports retain caller-owned objects shared with ISRs through critical sections; a fully interrupt-safe ownership model remains future work. SDK build success and model coverage do not establish electrical safety or physical reliability.

Continue with [boot and flash](boot-and-flash.md), [stock behavior](stock-behavior.md), [SDK builds](../development/firmware-builds.md), or [host simulation](../development/README.md).

Use the [firmware feature board](../../FIRMWARE_BOARD.md) for FW01–FW29 prerequisites, current acceptance scope, and remaining work.

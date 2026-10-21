# Firmware architecture and status

## Two independent implementations

The local server talks to **stock 1.1.15** over its existing MQTT protocol. It does not run inside the feeder. The replacement firmware uses Rust application components on the Realtek AmebaD SDK. Its current physical milestones include build-bound diagnostic startup, stopped-output GPIO startup, and bounded visible indicator control; functional feeding remains unimplemented on hardware.

| Crate | Responsibility |
| --- | --- |
| `petlibro-core` | `no_std` controller, portion accounting, sensor qualification, ADC scaling, motor gate, feedback queue, monotonic clock, journal, and flash authority |
| `petlibro-amebad` | GPIO/ADC board port, interrupt/timer supervision, watchdog, FreeRTOS owner, RAM parking, SDK flash hooks, build-bound startup reports, stopped outputs, and LED diagnostics; host fake SDK for tests |
| `petlibro-ffi` | C ABI and per-core static libraries joining Rust to SDK startup |
| `petlibro-testkit`, `petlibro-sim` | Fake NOR journal storage and deterministic host worker |
| `petlibro-probe` | Small non-actuating GET_CONFIG execution probe embedded into a fixed stock-image slot |

Minimal C hooks call the Rust entrypoint. The SDK still supplies startup, FreeRTOS, radio code, linker scripts, and image packaging. KM0 and KM4 are built separately. Header-derived bindings are generated for SDK-backed KM4 builds; host tests use fake bindings.

## Runtime contracts

The owner serializes requests and drives the controller from qualified inputs and captured feedback. The output gate uses stop intervals, leases, and fault latches. Timer supervision can stop drive when the owner stalls. Watchdog refresh requires progress. Interrupted feeding is reconciled conservatively rather than automatically resumed.

The journal uses alternating complete records, CRC, and a final commit marker. Tests exercise torn writes and restart behavior against fake NOR. A working physical flash adapter and verified storage allocation are still missing. Flash coordination, SDK hooks, and RAM parking are tested components; complete exclusive flash ownership across both cores is unfinished.

The core diagnostic uses synthetic inputs and memory-only motor outputs. Separately, the default application explicitly enables GPIO clocks, requests physical motor STOP/infrared-off, and runs a bounded LED probe. The minimal KM0 main restores the stock shared-pad supply prerequisite. The full feeding backend remains unopened. Voltage/transient acceptance and Port A input-readback faults remain unresolved; see [startup diagnostics](startup-diagnostics.md).

## Capability and acceptance map

| Area | Present | Hardware/integration work remaining |
| --- | --- | --- |
| Manual feeding and single/double bowls | Host-tested controller, direction/portion accounting, bounded recovery | Mechanical phase, bowl mapping, measured portions, jams, verified stops |
| Inputs and power | Host-tested filtering, stale/invalid-input rejection, ADC scaling | Calibration, optical timing/polarity, current limits, battery/brownout behavior |
| Request persistence | Host-tested journal and owner integration | Physical flash provider, durable writes, power cuts |
| Runtime reliability | Host-tested leases, ISR supervision, watchdog, and flash coordination | Real timing, reset/sleep behavior, sustained runs and complete flash mediation |
| Networking and local protocol | Stock server implemented separately | Replacement provisioning, networking, commands, telemetry, reconnect |
| Schedules | Stock semantics investigated | Replacement persistence, timezone/DST, occurrence identity, interrupted-meal policy |
| Panel and audio | Stock roles partially recovered | Replacement buttons/lock/reset, complete indicator policies, audio playback and deadlines |
| Home Assistant | Deferred | Interface and integration after useful standalone operation |

Rust ports retain caller-owned objects shared with ISRs through critical sections; a fully interrupt-safe ownership model remains future work. SDK build success and model coverage do not establish electrical safety or physical reliability.

Continue with [boot and flash](boot-and-flash.md), [stock behavior](stock-behavior.md), [SDK builds](../development/firmware-builds.md), or [host simulation](../development/README.md).

Use the [firmware feature board](../../FIRMWARE_BOARD.md) for FW01–FW29 prerequisites, current acceptance scope, and remaining work.

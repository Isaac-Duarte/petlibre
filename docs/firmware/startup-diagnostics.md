# Startup, stopped outputs, and indicator diagnostics

The current diagnostic initializes limited physical GPIO, then runs the synthetic feeding-core test and a bounded indicator experiment. It leaves the full `Board` backend, feeding owner, ADC/sensors, and networking unopened. An unopened backend does **not** mean that startup has no electrical side effects.

## Build-bound startup reports

The SDK wrapper hashes canonical JSON containing the profile and complete root-relative source-input hash map. This becomes `startup_identity` in the manifest and `PETLIBRO_BUILD_ID` in the firmware. It excludes timestamps; the final OTA hash remains a separate artifact identity. Target compilation of the port requires a lowercase 64-digit ID; host tests use `host`.

```text
PLFW1 start build=<64 lowercase hex digits> bank=1 reset=00000000 mmu=00000003,0c000000,03e7a000
PLFW1 result diagnostic=pass backend=unopened
```

Live RSIP mapping entry 0 identifies bank 0 or 1 when virtual `0x0c000000` maps to `0x08006000` or `0x08186000`; other mappings report unknown. Raw mapping and factory-preserved reset flags remain in the report. A zero reset word does not establish a particular physical reset cause.

`audit-firmware-startup.py` verifies the frozen OTA/manifest, embedded identity, finalized receive-only capture hashes, event coverage, actual bank mapping, ordering, and observation interval. Missing/wrong build, faults, incomplete or malformed records, repeated startup, and policy-specific failures are distinct failing outcomes. It emits fixed fields rather than arbitrary device text. Example using your own complete evidence bundles:

```sh
.venv/bin/python tools/audit-firmware-startup.py \
  --capture private/captures/normal-boot \
  --manifest private/verification/review/build-manifest.json \
  --image private/verification/review/image.bin \
  --minimum-seconds 60 \
  --output private/verification/startup-result.json
```

FW02 passed artifact-bound physical startup verification. This evidence does not establish long-term uptime, persistent boot counters, or a verified physical reset cause.

## Stopped-output startup

The initializer explicitly enables/checks LP GPIO clock/function bits while preserving unrelated controls. With the interrupt mask held/restored, it preloads motor PA13/PA14 HIGH/HIGH and infrared PA15/PB3 LOW/LOW before output direction. It verifies masked values and directions before scheduling diagnostics. Failure logs an error and does not start the diagnostic task. This requests the stock-derived STOP/off state; physical polarity and startup transients remain acceptance work.

`PLCLK1` records before/after clock/function words. `PLFW1 outputs` reports PA/PB values and directions before the startup banner. Manifest policies must match record formats in the hashed image. An output mismatch cannot be hidden by a synthetic diagnostic PASS.

FW03 has exact GPIO readback, artifact-bound normal boot, and operator-reported no-motion evidence. Independent voltage/infrared-off measurements, repeated measured resets, and transient checks were deferred to FW27. The inspected CON3 harness carries both motor leads and board power; disconnecting it is not an established way to isolate the motor while retaining board power.

## Bounded indicator probe

The controller preloads indicator latches LOW and selects stock-derived GPIO/no-pull configuration. It changes only PA0, PA2, PA4, and PB22. Every request requires valid indicator directions and stopped-output readback. Requests yield for 10 ms before observing the output latch, input, pad control, and direction state. Timing requests include the settle interval.

After startup and diagnostic PASS, the sequence waits nominally 3 seconds, then gives each pin two HIGH pulses in PA0/PA2/PA4/PB22 order: HIGH 1 s, LOW 0.5 s, HIGH 1 s, LOW 1 s. All lines are then requested LOW and the task ends. Logging/scheduling add time. There is no indefinite blinking, feeding request, or autonomous retry.

A HIGH input mismatch remains a failed observation while permitting only the already bounded visible interval; a LOW mismatch, changed latch/mux/direction, or lost stopped state aborts. Terminal `finish=input_mismatch low=all` is a failure, never a verified success. Loss of stopped readback requests STOP/off again.

| Record prefix | Meaning |
| --- | --- |
| `PLPWR1` | KM0 shared-pad supply before/after controls |
| `PLCLK1` | GPIO clock/function enable and preservation |
| `PLFW1` | Stopped outputs, source identity, bank/reset, diagnostic result/errors |
| `PLIO1` | Read-only pad-group/AON/GPIO routing observations |
| `PLDBG1` | Requested level, latch, input, directions, stopped state, pad control |
| `PLLED1` | Bounded sequence begin, individual results, terminal LOW/failure |

## Shared-pad prerequisite and current physical findings

The minimal KM0 main omitted a stock prerequisite for audio-shared pads used as GPIO. The port restores AMACRO and analog pad enables plus the SDK's calibrated `SOCPS_AudioLDO(ENABLE)` helper before entering the scheduler. It does not initialize audio playback. `trace-stock-pad-power.py` executes the stock initializer and LDO instructions against modeled registers/delays; its fixture tests distinguish LDO power from the separate pad-enable bit. This is instruction-level evidence, not an electrical supply measurement.

With that prerequisite restored, the operator observed all four indicators blink twice and finish dark: **PA0 Wi-Fi, PA2 low food, PA4 locked, PB22 warning**. The sequence establishes HIGH-on/LOW-off on the tested unit; only the warning's red color is recorded.

Port A HIGH requests still return a HIGH latch but LOW input despite visible light. PB22 readbacks pass. The captured two starts were intentional operator reboots, but the strict verifier correctly returns repeated startup rather than treating that capture as one accepted boot. KM0/KM4 UART log interleaving also prevents accepting some numeric reports as intact. Do not reconstruct a clean stream or relabel those failures. **FW04 remains in Bench check** pending readback diagnosis and serialized reporting; see the [feature board](../../FIRMWARE_BOARD.md).

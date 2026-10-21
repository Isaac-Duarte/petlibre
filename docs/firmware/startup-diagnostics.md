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

The initializer explicitly enables/checks LP GPIO clock/function bits while preserving unrelated controls. With the interrupt mask held/restored, it preloads motor PA13/PA14 HIGH/HIGH and infrared PA15/PB3 LOW/LOW before output direction. It verifies masked values and directions before scheduling diagnostics. Infrared OFF requires LOW output latches as well as LOW pad inputs, because a pad with a dead input path reads LOW whatever is driven (as PA0–PA7 do on the tested unit). Failure logs an error and does not start the diagnostic task. This requests the stock-derived STOP/off state; physical polarity and startup transients remain acceptance work.

`PLCLK1` records before/after clock/function words. `PLFW1 outputs` reports PA/PB values and directions before the startup banner. Manifest policies must match record formats in the hashed image. An output mismatch cannot be hidden by a synthetic diagnostic PASS.

FW03 has exact GPIO readback, artifact-bound normal boot, and operator-reported no-motion evidence. Independent voltage/infrared-off measurements, repeated measured resets, and transient checks were deferred to FW30 (electrical output verification, required before supervised motion). The inspected CON3 harness carries both motor leads and board power; disconnecting it is not an established way to isolate the motor while retaining board power.

## Bounded indicator probe

The controller preloads indicator latches LOW and selects stock-derived GPIO/no-pull configuration. It changes only PA0, PA2, PA4, and PB22. Every request requires valid indicator directions and stopped-output readback. Requests yield for 10 ms before observing the output latch, input, pad control, and direction state. Timing requests include the settle interval.

After startup and diagnostic PASS, the sequence waits nominally 3 seconds, then gives each pin two HIGH pulses in PA0/PA2/PA4/PB22 order: HIGH 1 s, LOW 0.5 s, HIGH 1 s, LOW 1 s. All lines are then requested LOW and the task ends. Logging/scheduling add time. There is no indefinite blinking, feeding request, or autonomous retry.

The 64 extra input samples distinguish a dead input path (0/64) from a marginal threshold (a partial count). They are informational: verification still uses the single input read, so a high count cannot verify a failed step.

PA0, PA2 and PA4 are **output-only** on the tested unit (owner scope decision after the Port A traces below). Their steps report `status=output` when latch, pad control, directions and stopped state agree, and the begin line names them (`PLLED1 begin output_only=PA0,PA2,PA4`). `output` is never `pass`: it does not establish an electrical level, which rests on the operator's visual observation. PB22 keeps full input verification. Policy `bounded-probe-v3` expects exactly this sequence. The auditor's sequence outcome is `pass`, but `led_probe_verified` stays `false` whenever output-only pins are present; `led_input_verified_pins` (`PB22`) and `led_output_only_pins` (`PA0`, `PA2`, `PA4`) state what was read back.

A HIGH input mismatch remains a failed observation while permitting only the already bounded visible interval; a LOW mismatch, changed latch/mux/direction, or lost stopped state aborts. Terminal `finish=input_mismatch low=all` is a failure, never a verified success. Loss of stopped readback requests STOP/off again.

| Record prefix | Meaning |
| --- | --- |
| `PLPWR1` | KM0 shared-pad supply before/after controls; policy `before-km4-v1` requires one intact record before any KM4 report, `v2` adds the LDO trim, `v3` the raw Port A input after each supply step; an image linking the `v3` record format is refused unless its manifest declares `before-km4-v3` |
| `PLCLK1` | GPIO clock/function enable and preservation |
| `PLFW1` | Stopped outputs, source identity, bank/reset, diagnostic result/errors |
| `PLIO1` | Read-only pad-group/AON/GPIO routing observations; `read-only-v2` adds KM4's view of the shared-pad supply control and the raw Port A input word |
| `PLDBG1` | Requested level, latch, input, directions, stopped state, pad control; `bounded-probe-v2` adds `highs=N/64` extra input samples |
| `PLLED1` | Bounded sequence begin, individual results, terminal LOW/failure |
| `PLFLS1` | FW06 KM4/KM0 boot handshake: HELLO authority session, READY peer session and flash-coordinator binding, injected peer change, terminal watch result; policy `fw06-hello-v1`; under `fw06-park-v1` only HELLO and READY |
| `PLPRK1` | FW06 stage C park round trip: start mode/marker/watchdog, per-batch park/release maxima and frozen/alive counts, injected acknowledge/release faults, done or error/skip; policy `fw06-park-v1` |

## FW05 supervision probe capture

Policy `fw05-probe-v1` binds `PLTMR1`/`PLWDG1` records in the image; the startup auditor refuses a mismatch either way and still treats a two-boot capture as `repeated_startup`. After the LED probe, a cold boot opens the stopped board and TIM2, logs 13 `PLTMR1 sample` snapshots (SYSTIMER raw, supervision ms, accepted TIM2 interrupts, ready/healthy/stopped), a 2 s busy stall, then arms the watchdog (`PLWDG1 arm`), feeds it for 15 s (`fed`), and withholds progress (`stall_begin`, `stalled`) until one watchdog reset. The following boot repeats startup and the LED probe, then prints only `PLWDG1 skip`.

Motion stays disabled. The probe opens only the board's outputs (motor STOP, emitters OFF); it never configures sensor inputs, reads efuse calibration or touches the LP-domain ADC, whose clock the minimal KM0 main does not enable. No drive or emitter is requested. Before arming, it writes a marker to SDK user backup register 1 and reads it back. Any boot that sees a nonzero marker or boot reason clears the marker and skips the probe, so a reset cannot repeat the watchdog test until power is removed. Whether the marker survives the platform reset, and which boot-reason bits the reset sets, are bench observations. If both were lost, the board would repeat the probe on each boot, roughly every 100 s, until power is removed. The UART loader remains available at power-on. Clock values are nominal: SYSTIMER and TIM2 at 32 768 Hz (SDK: 31 µs ticks) and a watchdog of 4095 × 32 counts at about 33.3 kHz (`WDG_Scalar`), bounded to 2.9–5.3 s by 25–45 kHz limits. The capture measures them.

`audit-firmware-supervision.py` takes the same `--capture/--manifest/--image/--output` arguments as the startup auditor. It splits the capture at the second `PLFW1 start`, runs every declared startup policy on both boots, checks ms/irq consistency against SYSTIMER ticks, the stall, the watchdog vendor word and marker, and the 2.5–7 s reset interval, and estimates SYSTIMER and watchdog clock rates from host receive times. Failures have distinct outcomes (`wrong_boot_count`, `boot1_*`/`boot2_*`, `supervision_sequence`, `timer_clock`, `timer_open`, `timer_samples`, `timer_consistency`, `systimer_rate`, `stall_failed`, `watchdog_arm`, `watchdog_feed`, `watchdog_stall`, `watchdog_no_reset`, `reset_timing`, `boot2_skip`). Host timestamps carry USB latency; the estimates are UART timing evidence and do not establish electrical outputs or the physical reset cause.

### FW05 bench result

One physical receive-only capture of a cold boot passed the supervision auditor, and the operator saw the LED sequence twice. Both boots passed every startup policy.

- **SYSTIMER:** about 32 752 Hz against host receive times over 60 s. That is −475 ppm from 32 768 Hz, within the host timestamp uncertainty of roughly ±0.1%. It only moved forward.
- **TIM2:** a 10.015 ms period, against 10.010 ms nominal for 328 ticks.
- **Stall:** a 2 s busy stall (65 536 ticks) kept TIM2 interrupts running, and supervision stayed ready, healthy and stopped.
- **Watchdog:** armed with vendor word `5601001f` and fed for 15 s with 60 checkpoints, without a reset. The loop that stopped making progress latched fault 4 (PROGRESS) and never refreshed again.
- **Reset timing:** the reset followed about 3.0–3.1 s after the last refresh. A `stalled ms=3015` line arrived in the same receive chunk as the next boot's ROM output. That implies an effective watchdog clock near 43 kHz rather than the nominal 33.3 kHz. This is inside the configured 25–45 kHz bounds but close to the upper one, so tighten the clock bounds and counter choice before any production configuration relies on them.
- **Next boot:** reported boot reason `00100000` (`BIT_BOOT_KM4WDG_RESET_HAPPEN`) and the surviving marker, then skipped the probe. Both reset-loop guards worked on this unit.

Not yet exercised on hardware: owner-lease expiry while a task stalls. The probe never ran owner-side processing, so `owner_ready` was never established. Motion-path stop timing belongs to FW19.

## FW06 boot handshake

After `PLLED1 done low=all`, KM4 prints `PLFLS1 hello authority=<16 hex> count=<n>`: its authority session is the boot count in the high word and a fresh low word. KM0 answers READY with its own session, and KM4 binds a flash coordinator only while outputs are stopped, printing `PLFLS1 ready peer=<16 hex> reply=0 phase=1 fault=0 inactive=1` (ACCEPTED, IDLE, no fault). The test build's KM0 republishes a new session about 10 s later to simulate a rebooted peer. KM4 then reports `PLFLS1 peer peer=<16 hex> reply=5 phase=8 fault=7 blocked=1` (FAILED, FAULT, PEER_LOST), with the new session's high word one above the first. After a 30 s watch from READY it prints `PLFLS1 done phase=8 fault=7 blocked=1`. One `PLFLS1 foreign` line may appear after HELLO. Failures print `PLFLS1 error timeout ms=<n>` or `PLFLS1 error clock`. Policy `fw06-hello-v1` is bound to the HELLO record format in the image both ways. It requires the LED probe policy and exactly one fully parsed HELLO, READY, peer and done record, in that order. It checks the session and field values, and the host receive times: READY within 2.5 s of HELLO, the peer change 9–12 s after READY, and done 29–32 s after READY. Outcomes are `flash_handshake_missing`, `flash_handshake_malformed`, `flash_handshake_failed`, `flash_handshake_order` and `flash_handshake_timing`. The session values are reported as fixed hex fields. This is UART protocol evidence: it does not show that any flash write path is safe.

## FW06 park round trip

Policy `fw06-park-v1` is bound both ways to the `PLPRK1 start mode=` record format in the image, and still requires the HELLO record format. Under it the startup auditor checks only the handshake part of each boot: one HELLO and one READY (one `PLFLS1 foreign` allowed), after `PLLED1 done low=all`, with the `fw06-hello-v1` field values and READY within 2.5 s of HELLO. It refuses `PLFLS1 peer` and `PLFLS1 done` records and requires a `PLPRK1 start` record after READY.

After READY, KM4 prints `PLPRK1 start mode=<0|1|2> marker=<hex8> watchdog=<pass|fail>` and four `PLPRK1 batch leases=<n> park_max=<ticks> release_max=<ticks> frozen=<n> alive=<n>` records at 25, 50, 75 and 100 leases. Park and release maxima are 32 768 Hz ticks, bounded by the 40 ms exchange limit (1310). A passing capture is one power-on with exactly three startups:

- **Boot 1** (mode 0) injects `PLPRK1 inject mode=0 lease=101 begin=0 ... ms=0`: KM4 stops acknowledging a park, and the watchdog resets 2–6 s later.
- **Boot 2** (mode 1) injects `inject mode=1 lease=101 begin=1 ... ms=0` and withholds the release. KM0 publishes TIMEOUT (`peer=504b0081`) after its 700 ms park budget, reported as a second inject line with `ms` within 650–760. The watchdog resets 2–6 s after the first inject line.
- **Boot 3** (mode 2) prints `PLPRK1 done leases=100 marker=00000002`, then nothing more from `PLPRK1`/`PLFLS1` for at least 20 s.

Every batch needs frozen equal to leases and alive at least leases − 1. HELLO counts must be 1, 2 and 3, and the KM0 peer session high words must be nonzero and strictly increasing. Failures print `PLPRK1 error lease=... stage=... reply=... phase=... fault=...`, `PLPRK1 error no_reset ms=<n>` or `PLPRK1 skip marker=<hex8>`.

`audit-firmware-park.py` takes the same `--capture/--manifest/--image/--output` arguments as the supervision auditor and requires `fw06-park-v1`. It splits each boot after its last `PLPRK1` line before the next `PLFW1 start` and runs every declared startup policy on each segment. Outcomes are `wrong_boot_count`, `boot1_*`/`boot2_*`/`boot3_*`, `park_sequence`, `park_start`, `park_batches`, `park_inject_ack`, `park_inject_release`, `park_reset_timing`, `park_done`, `park_sessions` and `pass`. It reports reset flags, HELLO counts, session high words, batch values, both reset intervals, the release timeout and the clean tail. This is UART timing evidence: host timestamps carry USB latency, and it does not establish the physical reset cause or that flash access was excluded during parks.

## Shared-pad prerequisite and current physical findings

The minimal KM0 main omitted a stock prerequisite for audio-shared pads used as GPIO. The port restores AMACRO and analog pad enables plus the SDK's calibrated `SOCPS_AudioLDO(ENABLE)` helper before entering the scheduler. It does not initialize audio playback.
Stock and the SDK run this after booting KM4; source after review 06 runs it
before `km4_boot_on()` so the pads are powered before KM4 configures them and
`PLPWR1` completes before KM4 logs. A bench capture of that order showed an
intact `PLPWR1` ahead of KM4 output, with `after_pad=00000301`: the LDO is
powered, but its voltage trim (bits 7:1) is zero. `SOCPS_AudioLDO(ENABLE)`
restores a trim saved only by its disable path, so a cold start gets zero. Stock
KM4 `CODEC_Init` and the SDK's `CODEC_Init` write 0x2A, or 0x68 on a 1.8 V supply.
The port now writes that trim after the enable, and policy `before-km4-v2`
requires it. With the trim applied, Port A inputs
still read LOW. A later build sampled raw Port A input after each KM0 supply step
and got the same word every time, with PA0–PA7 already reading 0 before the first
step. The shared-pad supply therefore does not explain the readback. PA13/PA14
and PB22 read back correctly; the low Port A pads appear to have no usable input
readback on the tested unit. `trace-stock-pad-power.py` executes the stock initializer and LDO instructions against modeled registers/delays; its fixture tests distinguish LDO power from the separate pad-enable bit. This is instruction-level evidence, not an electrical supply measurement.

With that prerequisite restored, the operator observed all four indicators blink twice and finish dark: **PA0 Wi-Fi, PA2 low food, PA4 locked, PB22 warning**. The sequence establishes HIGH-on/LOW-off on the tested unit; only the warning's red color is recorded.

Review 06's earlier capture (HIGH latch with LOW Port A input, two intentional operator boots, interleaved KM0/KM4 reports) keeps its failing verdicts; do not reconstruct a clean stream or relabel them. The output-only build later passed the auditor on a single boot: intact `PLPWR1` before KM4 output with trim 0x2A, PA0/PA2/PA4 `status=output`, PB22 `pass`, terminal LOW. **FW04 is Done within this output-only scope** (physical capture plus operator observation); see the [feature board](../../FIRMWARE_BOARD.md).

### FW06 bench results

These are physical results on the research unit; they concern UART-visible protocol and timing, with nothing written to flash.

- **Boot handshake (`fw06-hello-v1`).** Passed on one cold boot. KM0 answered within one receive chunk. A deliberately changed KM0 session latched PEER_LOST 10.0 s later, with outputs blocked.
- **Park round trip (`fw06-park-v1`).** One power-on produced three boots, as designed:
  1. The withheld ACK reset 3.91 s later.
  2. After the withheld RELEASE, KM0 published TIMEOUT at 705 ms, and the platform reset.
  3. A clean run followed.

  All 300 leases showed a frozen KM0 heartbeat while parked and an advancing one between leases. The worst park took 6 SYSTIMER ticks; release took 2.
- **KM4 SysTick CLKSOURCE.** It reads 1 whatever is written, so park and hook context checks test only ENABLE and TICKINT.
- **Watchdog clock.** The reset came about 4.0–4.2 s after the last rate-limited refresh, an effective clock near 31–33 kHz. FW05 measured about 43 kHz with the same counter, and the cause of the difference is unknown.

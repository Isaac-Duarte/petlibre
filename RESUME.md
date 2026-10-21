# Resume firmware development

Handoff checkpoint: **October 20, 2026, Mountain time**. Read this, then
[AGENTS.md](AGENTS.md), the [development guide](docs/development/README.md),
and [FIRMWARE_BOARD.md](FIRMWARE_BOARD.md). Update this handoff when the installed
image, bench state, next task, or acceptance evidence changes.

## Where we stopped

**FW05 is Done (motion-disabled scope). FW06 is next.** The FW05 supervision probe (OTA
`279c645c087e8333a58fc51edfe8a02e0bdc3df490ab488bfa7c9ff08b6d5188`, startup
identity `9e953f1c…e912`) is installed in bank 1. It went on through the
changed-sector updater: 30 sectors verified. One receive-only capture of a cold
boot through its deliberate watchdog reset passed `tools/audit-firmware-supervision.py`.
The motor was never commanded. Results are in
[startup diagnostics](docs/firmware/startup-diagnostics.md#fw05-bench-result).

- **SYSTIMER:** about 32 752 Hz, within host timing uncertainty of 32 768 Hz.
- **TIM2:** 10.015 ms.
- **Stall:** TIM2 interrupts kept running during a 2 s stall.
- **Watchdog:** fed for 15 s without a reset. The loop that stopped making
  progress never refreshed, and the reset came about 3.0–3.1 s after the last
  refresh. That puts the watchdog clock near **43 kHz**, not the 33.3 kHz assumed.
- **Next boot:** reported `00100000` (KM4 watchdog reset) and the backup marker,
  then skipped the probe.

FW05 handed three items to later cards: hardware lease expiry under a stall
(FW16), production watchdog bounds (FW06) and stop timing under motion (FW19).
The board's carried-items table tracks these. The board was also restructured:
FW30 (electrical output verification) now gates FW16, FW14 no longer waits on
FW06, and FW31 (bench automation) is available.

FW04 is Done (output-only scope):

A single artifact-bound boot of the output-only build passed the startup
auditor (physical capture, receive-only, 152 s observed after startup):
correct build/bank, stopped motor/infrared outputs, clock, complete routing,
an intact `PLPWR1` ahead of all KM4 reports with LDO trim 0x2A
(`share_pad=00000355`), and LED policy `bounded-probe-v3`.

What was resolved along the way:

1. **Interleaved reports.** KM0 now runs the shared-pad initializer before
   `km4_boot_on()` (stock and the SDK run it after), so `PLPWR1` completes
   before KM4 logs. Verified on hardware.
2. **LDO trim zero.** `SOCPS_AudioLDO(ENABLE)` restores a trim that only its
   disable path saves, so a cold start got zero. KM0 now writes the stock
   `CODEC_Init` value (0x2A, or 0x68 on a 1.8 V supply). Verified on hardware;
   it did not change the readback.
3. **Port A readback.** Raw Port A input sampled after each KM0 supply step
   read `e1c40e00` every time: PA0–PA7 read 0 before any supply change, even
   PA7 (`UART_LOG_TXD`, idles HIGH). PA13/PA14 and PB22 read back correctly.
   The low Port A pads have no usable input readback on this unit (physical
   observation plus register traces; cause not established). By owner decision
   PA0/PA2/PA4 are **output-only**: `status=output`, never `pass`, resting on
   latch/pad/stopped-state checks and visual observation. PB22 keeps input
   verification.

Earlier captures of review 06 and intermediate builds remain as recorded
(including review 06's intentional two-boot capture); keep raw evidence
unchanged. See [startup diagnostics](docs/firmware/startup-diagnostics.md).

## What is actually installed

The research feeder runs the **FW05 supervision probe** in bank 1, starting at
flash offset `0x186000`. It was installed over the FW04 output-only build
(OTA `947dc797…0f29`), which remains the previous image.

| Identity | Recorded value |
| --- | --- |
| OTA SHA-256 | `279c645c087e8333a58fc51edfe8a02e0bdc3df490ab488bfa7c9ff08b6d5188` |
| Embedded startup build ID | `9e953f1ce152233278cc226088a793d423a47920cf0da09e2390aa137f02e912` |

**Every cold boot of this image deliberately triggers one watchdog reset**, about
100 s in. Expect two startups per power-on. Its frozen review is the source of
the next update.

## Last confirmed physical state

This describes the last operator-confirmed session, not a live observation.
Reconfirm it with the operator before any bench operation.

- Scope is the **research PLAF103 only**. The existing household feeder is excluded.
- Feeder power and adapter USB connected; adapter TXD jumper disconnected;
  adapter RXD and common GND connected; breadboard button released.
- Black is feeder GND on the negative rail; grey is feeder TX at row 2a;
  white is feeder RX at row 1a. Feeder TX connects to adapter RXD.
- Loader entry uses feeder TX through a measured approximately 4.72 kΩ resistor
  and normally-open momentary button to GND. No direct TX-to-ground short.
- Adapter power/VCC and flow-control pins remain unwired. No batteries were in use.
- CON3 remains connected: this motor harness also carries board power.
  The front indicator PCB is difficult to access; no disassembly or wire cutting
  was needed for the successful LED test.

Consult the [UART procedure](docs/hardware/uart.md) before changing wiring.
The old loader/helper session is finished; historical receipts are not resumable.

## Evidence available in this local checkout

Start with **`private/bench-handoff/current.json`**. It indexes the exact local
installed review, frozen build, session, physical result, boot capture, verifier
result, installation audit, baseline, ROM report, and archived research snapshot.
Its file hashes identify the indexed evidence. Dated paths and bench-specific
details stay in that ignored file rather than becoming public fixture conventions.

A fresh Git clone does not contain `private/`, vendor images, captures, or device
backups. It can support ordinary development through the documented setup/checks;
continuing this particular bench session requires the private evidence bundles.
Their absence does not require inventing a new baseline or weakening writer checks.
See [fixtures](docs/development/fixtures.md) and [preserved research](docs/development/provenance.md).

## Review fixes included since FW04

These review fixes landed on top of `c8bb49f` with static and host evidence. The
installed FW05 probe build includes them. The FW05 capture exercised only the
timer-repeat and stopped-output paths; the others have no hardware evidence:

- Infrared OFF requires LOW output latches as well as LOW inputs, because Port A
  input readback is not trustworthy. PA15/PB3 readback is still unmeasured.
- A determined initialization or recovery fault holds verified STOP and keeps
  refreshing the watchdog instead of reset-looping; a hang still resets.
- The owner refuses flash limits whose total lease can exceed the watchdog
  progress gap. Production limits need measured erase/program times (FW06).
- One timer IRQ without systimer advance is tolerated; two consecutive fault.
- `Events::close()` turns infrared off; the KM4 panic handler requests STOP
  and infrared off; SysTick checks ignore COUNTFLAG.
- `build.py` refuses SDK worktree drift outside an allowlist, Rust build
  overrides, and toolchain file-hash mismatches. The `plaf103` and
  `plaf103-firstboot` private builds were rebuilt afterwards.

## Next work and retained decisions

FW06 (real flash coordination) is next. It also fixes the production watchdog
configuration from the FW05 measurement. Build and freeze each candidate, install through the
changed-sector updater after the operator enters loader mode, and capture boots
receive-only. Start the recorder and confirm it holds the port before the
operator powers on; a capture that begins mid-boot cannot be audited.
Do not silently turn a failed input observation into a verified electrical level.

Retain the owner's development preferences:

- Use the fast [changed-sector UART updater](docs/development/uart-development.md).
  Routine iterations do not require a new full-flash read or stock round trip.
  Keep fresh helper/source/guard/status checks, exact plans, and readback verification.
- Preserve the demonstrated conservative recovery path. Stock return through the
  newer development updater remains untested; no automatic retry/replay is allowed.
- OTA can wait because UART iteration is faster now; it remains planned for
  later autonomous development iterations.
- FW03 electrical voltage, infrared-off, measured-reset and transient checks were
  deferred to FW30, which now gates supervised motion; visible LEDs and software STOP readback do not complete them.

These are recorded scope decisions, not authorization to operate hardware or
publish changes. Follow the current user task and repository instructions.

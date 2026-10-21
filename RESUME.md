# Resume firmware development

Handoff checkpoint: **October 20, 2026, Mountain time**. Read this, then
[AGENTS.md](AGENTS.md), the [development guide](docs/development/README.md),
and [FIRMWARE_BOARD.md](FIRMWARE_BOARD.md). Update this handoff when the installed
image, bench state, next task, or acceptance evidence changes.

## Where we stopped

**FW04 is Done (output-only scope). FW05 is next.** The operator observed
Wi-Fi → low food → locked → warning, each blinking twice, then all dark.
Mapping: PA0 Wi-Fi, PA2 low food, PA4 locked, PB22 warning; HIGH on, LOW off.
Only the warning's red color is explicitly recorded.

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

The research feeder runs the **output-only build** in bank 1, starting at
flash offset `0x186000`, installed over the Port A trace build by the
changed-sector updater (18 sectors verified).

| Identity | Recorded value |
| --- | --- |
| OTA SHA-256 | `947dc797876509d42dfc0c8c1d7e6630525cdd56cc0988684596ef9ea8520f29` |
| Embedded startup build ID | `5f00cb16bb88d4baaff6145ae4a963b0af087296fe518548751700979151168d` |

Its frozen review is the source of the next update. New builds change source
identity; freeze each candidate separately and never rewrite old manifests.
The updater still checks the actual current application before accepting a plan.

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

## Next work and retained decisions

FW05 (clock, independent stop timer, watchdog) is next; initial stall tests run
with motion disabled. Build and freeze each candidate, install through the
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
  deferred to FW27; visible LEDs and software STOP readback do not complete them.

These are recorded scope decisions, not authorization to operate hardware or
publish changes. Follow the current user task and repository instructions.

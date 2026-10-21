# Resume firmware development

Handoff checkpoint: **October 20, 2026, Mountain time**. Read this, then
[AGENTS.md](AGENTS.md), the [development guide](docs/development/README.md),
and [FIRMWARE_BOARD.md](FIRMWARE_BOARD.md). Update this handoff when the installed
image, bench state, next task, or acceptance evidence changes.

## Where we stopped

**FW04 is in Bench check. All four physical indicators work.** The operator
observed Wi-Fi → low food → locked → warning, each blinking twice, then all
remaining dark. Mapping: PA0 Wi-Fi, PA2 low food, PA4 locked, PB22 warning;
HIGH on, LOW off in this bounded experiment. Only the warning's red color is
explicitly recorded.

The three Port A indicators became visible after restoring the stock/SDK
shared-pad supply initializer in minimal KM0 startup. The current diagnostic
requests motor STOP and infrared off, runs a synthetic feeding-core test, then
the bounded LED sequence. The full feeding backend and networking remain unopened.

Two issues remain:

1. PA0/PA2/PA4 HIGH requests have a HIGH output latch but a LOW input readback,
   despite visible light. PB22 readbacks pass. Both captured sequences terminate
   LOW with `finish=input_mismatch low=all`; the diagnostic has not fully passed.
2. KM0's `PLPWR1` supply report interleaves with KM4's `PLCLK1` clock report.
   Those numeric records cannot be accepted as intact evidence.

The capture contains two expected build/bank startups. The operator explicitly
rebooted to write down the sequence; do not infer a spontaneous boot loop. The
strict verifier returns `repeated_startup`, retaining the LED failures. Keep the
raw evidence unchanged. See [startup diagnostics](docs/firmware/startup-diagnostics.md).

## What is actually installed

The research feeder last ran **review 06**, a 114,688-byte application payload
in bank 1, starting at flash offset `0x186000`.

| Identity | Recorded value |
| --- | --- |
| OTA SHA-256 | `f19f9ed2df19b59dd83a8bb8b65b878561facdd8b3bf1ab06966a5dc0dd05093` |
| Embedded startup build ID | `9d2cc4de2fbe1ff57bb3789c659121fb3fd7b50bf485918d4cc06f72c5b56b17` |

The installation independently verified 12 changed sectors, three application/
guard snapshots, 25 status pairs, and the exact transmitted commands in 109.1 s.
Visible LED acceptance and successful installation are distinct from full
startup-verifier acceptance.

**Current Wellfed source and new builds are not the installed artifact.**
Migration changed source/input identities. Preserve the frozen installed review
as the source of any next update; build and freeze a separate new target.
Do not rewrite old manifests to match current source. The updater still checks
the actual current application before accepting a plan.

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

Start offline: investigate the shared-pad input-readback disagreement and
serialize startup reporting. Relevant code is
`firmware/rust/amebad/src/leds.rs`, `firmware/rust/amebad/src/sdk/target.rs`,
`firmware/ports/amebad/firstboot_km0.c`, and `tools/audit-firmware-startup.py`.
Do not silently turn a failed input observation into a verified electrical level.
No additional reboot is needed to establish the already recorded visible result.

After focused changes and applicable checks, build/review a separate candidate.
A later operator-supervised bench check should capture an unambiguous single boot,
intact reports, and the bounded sequence. Keep FW04 in Bench check until its
remaining acceptance work is resolved. FW05 timer/watchdog work follows; the board
tracks the longer feature plan.

Retain the owner's development preferences:

- Use the fast [changed-sector UART updater](docs/development/uart-development.md).
  Routine iterations do not require a new full-flash read or stock round trip.
  Keep fresh helper/source/guard/status checks, exact plans, and readback verification.
- Preserve the demonstrated conservative recovery path. Stock return through the
  newer development updater remains untested; no automatic retry/replay is allowed.
- Finish this LED work before moving on. OTA can wait because UART iteration is
  faster now; it remains planned for later autonomous development iterations.
- FW03 electrical voltage, infrared-off, measured-reset and transient checks were
  deferred to FW27; visible LEDs and software STOP readback do not complete them.

These are recorded scope decisions, not authorization to operate hardware or
publish changes. Follow the current user task and repository instructions.

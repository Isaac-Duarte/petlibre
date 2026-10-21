# Resume firmware development

Handoff checkpoint: **October 20, 2026, Mountain time**. Read this, then
[AGENTS.md](AGENTS.md), the [development guide](docs/development/README.md),
and [FIRMWARE_BOARD.md](FIRMWARE_BOARD.md). Update this handoff when the installed
image, bench state, next task, or acceptance evidence changes.

## Where we stopped

**FW15 (durable settings and feeding journal) is in Bench check. FW06 is done (mediated-path scope).**

The owner allocated `0x340000–0x350000` (64 KiB) for replacement-firmware storage.
It is blank in the backup and not referenced by stock 1.1.15, and it lies clear
of both banks and of the stock settings:

| Sectors | Use |
| --- | --- |
| `0x340000`, `0x341000` | Journal slots |
| `0x342000`, `0x343000` | Settings slots (store not yet written) |
| Remaining 12 | Reserved |

**Journal on NOR (physical, passed).** The feed journal ran through mediated
erase/program and mapped reads. One power-on gave three boots:

1. Create the journal; reserve and finish requests 1 and 2 (recorded honestly
   as CANCELLED before motion; nothing moved); reserve request 3; reset.
2. Request 3 read back pending and request 4 was refused; a torn record (half
   a record, no commit) went into the other slot; reset.
3. The journal reopened past the torn record, still pending and still
   refusing, then wiped blank.

Every lease froze KM0. Journal writes took 19–25 ms.

**Remaining for FW15:**

- the settings store;
- a physical power cut during writes (the reset and torn record above were
  deliberate injections);
- enforced wear bounds (one sector erase per journal write, about two per
  feed).

The FW06 plan has five stages, each with a write-free bench check until the last:

- **A, production constants (host).** `config.rs` defines the production
  watchdog and a 700 ms flash-lease budget. The owner's sample period must
  then stay at or under 139 ms.
- **B, boot handshake (physical, passed).** KM4 announces a fresh session
  (HELLO); KM0 answers with its own (READY). KM4 binds the coordinator only after
  reading back STOP and infrared OFF. A deliberately changed KM0 session
  latched PEER_LOST with outputs blocked. Backup registers were cleared by
  power loss.
- **C, park round trip (physical, passed).** One power-on produced three
  boots:
  1. 100 leases, then KM0 withheld its ACK; the watchdog reset 3.9 s later.
  2. 100 leases, then KM4 withheld its RELEASE; KM0 published TIMEOUT
     after 705 ms (budget 700), and the watchdog reset.
  3. 100 clean leases, then idle with the watchdog fed.

  Across every lease, KM0's heartbeat stayed frozen while parked and advanced
  between leases. The worst park took 6 ticks (about 0.2 ms) and the worst
  release 2. No flash was touched; the park loops run from RAM on both cores.
- **D, mediated read (physical, passed).** KM4 installed its lease provider in
  the SDK flash-lock hooks, then read the JEDEC ID 100 times through the SDK's
  RAM `FLASH_RxCmdXIP` (lock, ROM read-ID in SPIC user mode, unlock). All
  reads returned `c84017` in 100 leases, with KM0 frozen during each. The
  coordinator stayed idle and unfaulted, and the platform did not reset.
- **E, scratch write (physical, passed; owner-approved sector `0x185000`).**
  1. The ID read `c84017` and the sector read blank (1024 words).
  2. Three cycles each erased to blank, programmed 16 pages and verified with
     zero errors.
  3. The final erase left the sector blank.

  All 53 leases froze KM0. Sector erase took 27–35 ms, including lease
  overhead, against the 500 ms active budget; page programs took at most 0.7 ms.

Bench lessons from stage C:

- **SysTick on KM4.** CLKSOURCE (bit 2) is read-only 1 on the Cortex-M33, so
  the park and hook context checks now test only ENABLE and TICKINT. The
  narrower mask refused every park in the first stage C build; diagnostics
  located the cause.
- **Watchdog clock.** The resets came about 4.0–4.2 s after the last refresh,
  an effective clock near 31–33 kHz. FW05 measured about 43 kHz with the same
  counter, and the cause is unknown. The production bounds are now 24–54 kHz in
  source; the installed image predates that change.
- **Interrupted capture.** One capture was cut off by a host crash and is
  kept only as an observation. The first audit of the good capture hit an
  auditor timing bug, now fixed; both results are retained.

## What is actually installed

The research feeder runs **FW15 journal review 01** in bank 1, starting at flash
offset `0x186000`. It was installed through the changed-sector updater (22
sectors verified).

| Identity | Recorded value |
| --- | --- |
| OTA SHA-256 | `f046dc5d3297cdee94886bef1434673e4d7700444f10490754fd933c91acc62d` |
| Embedded startup build ID | `d08bf75d1b95f028c459ff1d6c2fab3e58ecb1becf312707f09a3101604eb70f` |

**Every cold power-on runs the three-boot journal test**:

1. It requires the region to be blank, then creates and exercises the journal.
2. It deliberately resets twice.
3. It wipes the journal sectors.

It never commands motion. Its frozen review is the source of the next update.

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

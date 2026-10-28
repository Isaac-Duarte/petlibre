# UART backup and recovery

This page covers backing up the PLAF103 over UART and restoring stock firmware with our pinned writers. It is for developers. If you only want to go back to stock from Wellfed, do it over the air instead; see [consumer installation](../../firmware/consumer-installation.md).

The writers here are pinned to our research unit's baseline and one reviewed diagnostic image. **They are not general PLAF103 installers.** A new backup, or a freshly compiled diagnostic, will normally fail their exact hash checks. Wellfed does not ship the private binary inputs they need.

## What has been shown

| Operation | Status |
| --- | --- |
| Enter download mode without the stock application | Works |
| Full 8 MiB read, repeated reads matching | Works |
| Bounded write-back of body sectors | Works |
| Marker activation with preservation checks | Works |
| Install and boot the UART-only diagnostic image | Works |
| Restore stock 1.1.15 to bank 1 and boot it | Works |
| Restore a full bank | Not verified on hardware |
| Recover from a power cut mid-write | Not verified on hardware |

The diagnostics also showed build-bound startup, physical stop and off requests, and bounded indicator control. None of this validates motor operation, schedules, networking, watchdog timing or long-run reliability.

## The tools

| Tool | What it does |
| --- | --- |
| `wellfed-flash read` | Restricted inspection and read transport |
| `wellfed-flash decode-status` | Decodes the flash protection registers |
| `wellfed-audit backup` | Compares independent backups |
| `wellfed-flash plan-stock-recovery` | Builds a stock-recovery plan offline |
| `wellfed-flash stock-recovery` | Runs a reviewed plan against bank 1. It refuses a target bank that can still boot. |
| `wellfed-flash inspect-journal` | Verifies a write journal |
| `wellfed-flash` emulated fixture tests (`wellfed-emu`) | Run the pinned helper on synthetic NOR: update, give-way, stock recovery and a fault mid-recovery. No hardware. |

## How the writers protect you

- Every writer needs a source-bound plan, evidence of the current image, an explicit serial identity, immutable inputs and full verification of the bytes before the write.
- Each sector write records its intent durably and is read back exactly.
- Activation holds back the first eight marker bytes until the staged image and every preserved range match. The final marker write adds no erase.
- A lost ACK or an uncertain write stops the run. Nothing retries or resumes on its own.
- Bytes outside the allowed application region are preserved, including bytes nobody has classified.

Do not reach for a blanket chip erase, protection clearing, an arbitrary image or a disabled check. Matching hashes do not prove you have the same physical device.

## Restore stock over UART

The [relay bench](bench.md#to-stock-over-uart) wraps these steps as `stock-prepare` and `stock-execute`. Run by hand, they are:

1. Run `wellfed-flash stock-recovery --help` with no device attached, and read the [fixture requirements](../../development/fixtures.md).
2. Enter download mode ([UART](uart.md#enter-download-mode)), then take a full read.
3. Check bank 1's marker. If bank 1 can still boot, give way to bank 0 first, because `stock-recovery` refuses a valid target marker.
4. Run `wellfed-flash stock-recovery prepare`. It writes a plan from the pinned baseline backup.
5. Review the plan and note its SHA-256. This is your last check before anything is written.

   > [!WARNING]
   > The next step rewrites bank 1. Keep independent copies of your full backups, and keep the feeder powered from here until the run ends.

6. Run `stock-recovery execute` with that plan hash. It reads all 8 MiB three times, so allow about an hour.
7. Boot the feeder normally, with the strap released. You should see stock's `app_main:version:` banner.

The boot capture holds the unit's serial number and cloud keys. Keep it private.

## Rules that don't bend

- Old plans and captures never authorize a new session.
- Changing the pinned profile needs its own reviewed compatibility and recovery design, plus hardware acceptance. Never edit a pinned hash to get an input past the writer.
- No contributor check opens a serial port. No SDK build flashes an image or starts the feeding owner.

The separate [UART development updater](../../development/uart-development.md) accepts frozen, reviewed source and target images within the tested bank layout. It verifies application regions and guard ranges rather than scanning all flash, and it does not relax these writers. Going back to stock through that updater is untested.

## Next steps

- [Flash layout](flash-layout.md): which ranges the writers preserve.
- [Bench](bench.md): the relay commands that wrap these steps.

## Evidence

- **Observed:** independent loader entry, bounded body-sector write-back, marker activation with preservation checks, a small stock-image Rust probe, and the UART-only diagnostic's install and boot. Exact stock restoration followed by a normal stock 1.1.15 boot completed the conservative round trip. These ran with the former Python clients, whose plan, journal and wire formats `wellfed-flash stock-recovery` keeps.
- **Observed, October 23, 2026:** `wellfed-flash stock-recovery` restored bank 1 on the research unit. A give-way came first, then a full read, a reviewed plan and its execution. Stock 1.1.15 booted from bank 1, and the reference build went back over UART.

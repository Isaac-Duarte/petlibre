# FW09 bank selection and boot policy

Status: Done. Selection, compatibility and activation were established offline (emulation of the research unit's actual factory loader, and static analysis of stock 1.1.15), then confirmed on hardware on October 22, 2026.

## How the factory loader chooses (simulation)

`petlibre-emu`'s factory module (formerly `audit-factory-mapping.py`) executes the factory loader's own instructions, taken from the verified 8 MiB backup, with the candidate B-cut ROM's mapping routines. The ordinary selection slice (`0x080003f4`) runs with this unit's initialized configuration: no selection or firmware-check callback, and the force-bank GPIO disabled (`0xff`).

| Bank 1 (`0x186000`) | Bank 0 (`0x006000`) | Boots |
| --- | --- | --- |
| valid | invalid | bank 1 (the unit's state today) |
| valid | valid | **bank 1**, whatever either image holds |
| invalid | valid | bank 0 |
| invalid | invalid | neither: a wait loop at `0x08000482`, no automatic UART fallback |
| KM0 valid, KM4 invalid | — | neither: a warning loop at `0x08000182`, the other bank is not tried |

The instructions explain the priority. With the force GPIO disabled, the loader checks bank index 1 first (`0x08000456`) and returns it if its signatures pass. Only on failure does it check bank 0 (`0x0800043a`). **Bank 1 has fixed priority; bank 0 is only a fallback.** The both-valid row held for stock in both banks, for our image in both, and for each mix. The audit now requires it.

A bank counts as valid when its image starts with the 8-byte marker `81958711` (ASCII), followed by valid KM0 and KM4 signatures. Stock 1.1.15's activation (`0x0e02a648`) writes the new image's marker last, then clears the first byte of the opposite bank's marker. Given bank 1's priority, that clearing is what lets stock move from bank 1 to bank 0.

## Capacities

| Bank | Start | Reviewed limit | Space |
| --- | --- | --- | --- |
| 0 | `0x006000` | `0x186000` (bank 1 start) | 1.5 MiB |
| 1 | `0x186000` | `0x2d6000` (stock footprint, the development updater's bound) | 1.3125 MiB |

Bank 1 may extend further: the next known use is the replacement storage at `0x340000`. Unknown gaps are not free space, though, so the reviewed bound stands. Replacement images are 684,032 bytes (`blink`) to 868,352 bytes (`feeder` with BLE reachable), 50–63 % of the smaller bank ([resource budget](resource-budget.md#flash)). The factory mapper sizes its windows from the image headers, rounded up to 4 KiB. KM4 maps at `0x0e000000` and KM0 at `0x0c000000` in either bank.

## Compatibility

Both banks map to the same virtual addresses: the mapper translates each bank's physical start to `0x0c000000` (KM0) and `0x0e000000` (KM4). One image therefore runs from either bank without relinking. The emulated mapping is identical for stock and for the probe image in both banks, and stock's updater writes the same container to either one.

Physically, the same replacement image has run from both banks ([hardware result](#hardware-result)), and every OTA since FW10 boots the recovery role from bank 0. The `device` build profile's OTA-address header matters only to the SDK's own OTA writer, which the replacement does not use.

## Activation rules for the replacement

From the selection logic:

1. **The candidate goes into bank 1, with its marker written last** after the rest of the image is verified. This is what the development updater does today.
2. **A known-good image in bank 0 is a fallback,** not a peer. It boots only when bank 1's marker or signatures fail.
3. **Never leave both banks invalid.** The loader then waits forever, and only the UART loader recovers. Any marker write must keep at least one valid bank.
4. **Clearing bank 1's marker is how to give way to bank 0.** Bank 0 cannot win while bank 1 is valid.

## Failed-first-boot recovery

The loader offers none of its own. An image with a valid marker that crashes after boot is selected again on every reset. Two mechanisms are available:

- **UART (proven):** the ROM loader is in ROM and independent of flash, so the relay bench can always enter it and reinstall bank 1. Today's two `--interrupted` recoveries used this path.
- **Firmware-driven fallback (design, for FW11):** bank 0 holds a confirmed known-good replacement image. A new candidate in bank 1 starts unconfirmed:
  - **Count:** early on every boot, before KM4 starts, KM0 counts unconfirmed boots in a backup register. Backup registers survive the watchdog reset but not power loss, so a power cut only restarts the count.
  - **Give way:** after a bound (for example three), KM0 clears the first byte of bank 1's marker, one page program before any other core runs, and resets. The loader then boots bank 0.
  - **Confirm:** the candidate confirms itself only after its startup report, stopped outputs, Wi-Fi and the local transport are healthy.

  Limits: this only helps if the candidate's KM0 code works. A broken KM0 or a broken factory configuration still needs UART. The FW05 watchdog is what turns a KM4 hang into a counted reset.

## Hardware confirmation (needs the owner)

The owner authorized writes anywhere in flash (October 22). The tools now cover each step with the bank-1 updater's checks, and the factory region stays unwritable ([updater](../development/uart-development.md)):

1. **Both valid:** `petlibre-bench flash-prepare/flash-execute --bank 0` installs a reviewed replacement image into bank 0. It overwrites the pinned baseline contents there (an invalidated marker), with bank 1 required valid throughout. The boot must report bank 1.
2. **Fallback:** `petlibre-bench give-way-prepare/give-way-execute --fallback <bank-0 review>` clears bank 1's first marker byte. It runs only while bank 0 is exactly that review. The boot must report bank 0.
3. **Restore:** `petlibre-bench flash-prepare/flash-execute --restore-marker` rewrites bank 1's first sector, marker last. The boot must report bank 1 again.

Each step ends with a full-flash read, compared with the one before it, and the UART loader path stays available throughout. The chained sequence passed in emulation: the real helper instructions performed each write, and the factory loader's own selection code picked each boot (the former `src/tools/tests/test_development_bank0.py`, removed with the Python updater on October 22, 2026). It then passed on the research unit with both clients (FW09, FW32).

## Hardware result

Physical, research unit, October 22 2026, through the relay bench. The same replacement build (startup identity `8d67a407…`) was used in both banks:

| Step | Write | Boot report |
| --- | --- | --- |
| A | That image into bank 0 (145 sectors, marker last), over the invalidated October 19 probe residue | `bank=1`: both valid, bank 1 kept |
| B | Give-way: bank 1's first marker byte cleared (one frame, no erase), with bank 0 read in full and equal to the review | `bank=0`, MMU offset `03ffa000`: the fallback works, and the image runs unchanged from bank 0 |
| C | Restore: bank 1's first sector rewritten, marker last | `bank=1`, MMU offset `03e7a000` |

Every step verified its readback, and every boot passed stopped-output startup and ran the LED and optics probes. No loader NAK occurred. Full-flash reads before step A and after step C differ only in bank 0's application region (`0x6000`–`0x97000`). Bank 1, the factory region, settings and storage are byte-identical.

This is also the first physical run of the replacement firmware from bank 0. The design's recovery primitive works: clearing bank 1's marker makes the loader boot bank 0. It is the step FW11's trial counter will take.

## Limits

- **Simulation, not the full reset:** this is the selection slice from supplied state, not a full reset or a power cut mid-write. It does not cover force-GPIO, non-null callbacks, security checks beyond the observed null callback, or damaged factory configuration.
- **Sampled ROM correspondence:** the candidate ROM matches sampled device words; its mapping routines were not among the words verified physically.

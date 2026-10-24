# Realtek RTL8721DM (AmebaD)

The SoC in the PLAF103, and the one Wellfed's SDK targets. Facts here hold for the chip and its SDK; how a given board wires them is on that [device's pages](../../devices/README.md). Labels follow the [evidence levels](../../evidence.md).

## Cores and SDK

- **Two cores.** KM0 (the low-power core) and KM4 (the application core) run separate images built separately. Wellfed's KM0 image contains only `wellfed-hal-common`; the application and the Rust runtime run on KM4 ([firmware architecture](../../firmware/README.md)).
- **The SDK** supplies startup, FreeRTOS, the radio stack, linker scripts and image packaging. Wellfed pins it under [`src/firmware/vendor/`](../../../src/firmware/vendor/README.md) and calls it from Rust through `wellfed-amebad-sys` (recovered).
- **Bluetooth is LE only.** The SDK's `bt_flags.h` sets `F_BT_BREDR_SUPPORT 0`, and the stack has no A2DP or LE Audio (recovered; see [Bluetooth](../../firmware/bluetooth.md)).

## Pins

The SDK encodes a pin as `(port << 5) | pin`, so `PA13` is 13 and `PB3` is 35. Some pins have a second fixed role that the board must release first. `PB3` is also `SWD_CLK`, and `PA7`/`PA8` are the LOGUART console (recovered). The [pin table](pins.md) lists every bonded pin with its SDK functions and each device's use of it.

## Memory map

From the linker scripts of the pinned SDK (`project/realtek_amebaD_va0_example/GCC-RELEASE/project_{hp,lp}/asdk/rlx8721d_layout*.ld`; recovered). The KM4 (`project_hp`) TrustZone layout is shown; the non-TrustZone `_is` variant differs only in the secure RAM split.

| Region | Start | Size | Notes |
| --- | --- | --- | --- |
| KM0 ROM | `0x00000000` | 96 KiB | Plus 16 KiB data ROM at `0x00020000` |
| KM0 SRAM | `0x00080000` | 64 KiB | ROM BSS, boot loader RAM, KM0 main RAM, MSP |
| Retention SRAM | `0x000C0000` | 1 KiB | Survives deep sleep |
| PSRAM | `0x02000000` | 4 MiB | If the package has it; first 32 bytes reserved |
| External flash (mapped) | `0x08000000` | Up to the part | Physical offsets below are relative to this base |
| KM0 image 2 (virtual) | `0x0C000000` | 32 MiB window | 32-byte header reserved; mapped onto a flash bank |
| KM4 image 2 (virtual) | `0x0E000000` | 32 MiB window | 32-byte header reserved; stock code addresses such as `0x0e0091ac` fall here |
| KM4 SRAM | `0x10000000` | 512 KiB | ROM BSS, MSP, 456 KiB main non-secure RAM, secure RAM, boot loader RAM |
| Extension SRAM | `0x100E0000` | 128 KiB | |
| KM4 ROM | `0x10100000` | 256 KiB | Plus 96 KiB data ROM at `0x101C0000` |

The SDK reserves the first 24 KiB of flash for boot material:

| Flash offset | Size | SDK role |
| --- | --- | --- |
| `0x0000` | 8 KiB | KM0 boot (XIP), 32-byte header |
| `0x1000` | 4 KiB | DPK calibration data (KM0 layout; overlaps the KM0 boot region's second page) |
| `0x2000` | 4 KiB | Backup system data |
| `0x3000` | 4 KiB | XIP system data |
| `0x4000` | 8 KiB | KM4 boot (XIP), 32-byte header |

This matches the PLAF103's `0x000000–0x006000` pre-application range ([flash layout](../../devices/plaf103/flash-layout.md)). The SDK layout says where the SDK would put these, not what a factory image holds; a device's own backup is the authority.

## Boot and download mode

- **ROM download mode.** The boot ROM enters UART download mode when the LOGUART TX line, `PA7`, is held low at cold power-on (recovered from manufacturer/SDK analysis). An SDK comment naming `PA30` conflicts with that. The PLAF103 entry that was actually tested is on its [UART page](../../devices/plaf103/uart.md).
- **Factory loader and banks.** A factory loader in the first flash sectors chooses between two application banks by their validity markers. The bank offsets and selection order a given device uses are on its flash-layout page, e.g. [PLAF103](../../devices/plaf103/flash-layout.md) and [bank policy](../../firmware/bank-policy.md).

## External flash

- External SPI NOR is mapped for execution at `0x08000000`. Flash offsets and mapped addresses are different coordinate systems, and so are OTA container offsets.
- Flash reads can be decrypted on the fly (OTF). A RAM helper that reads flash changes controller state, including the OTF configuration (recovered; see [UART backups](../../devices/plaf103/uart.md#backup-boundaries)).
- **OTP, eFuse and security registers are separate from external flash.** A full external-flash backup does not include them.

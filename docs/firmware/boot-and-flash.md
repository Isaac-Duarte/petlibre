# Boot and external flash

The inspected unit uses dual-core AmebaD startup with factory loader/configuration before the application banks. Stock 1.1.15 loads KM0/KM4 image regions and selects an application bank using validity markers; the factory loader checks bank 1 first and falls back to bank 0 ([bank policy](bank-policy.md)). Actual loader instruction execution with modeled ROM/MMIO accepts the reviewed diagnostic in both bank fixtures; it is not a full reset-path or power-cut simulation.

[layout.json](../../src/firmware/devices/plaf103/layout.json) contains the machine-readable stock-derived application layout. Values below are flash offsets with exclusive ends; mapped external flash begins at `0x08000000`.

| Range | Known role |
| --- | --- |
| `0x000000–0x006000` | Factory/pre-application material; preserve |
| `0x006000` | First application-bank start |
| `0x186000` | Second application-bank start; stock recovery target |
| `0x186000–0x2d6000` | Reviewed stock second-bank footprint |
| `0x3e8000–0x3f0000` | Observed settings sectors |
| `0x400000–0x401000` | Observed audio-start sector |
| `0x7cb000–0x7cc000` | Observed log sector |
| `0x7d0000–0x7d7000` | Observed plan-record sectors |
| `0x7da000–0x7db000` | Observed error-record sector |

This is not a complete partition map. Unknown gaps are not free storage. Physical flash extends to `0x800000`; separate OTP/eFuse/security registers are outside a normal external-flash backup.

The pinned stock OTA is 1,376,288 bytes including a 32-byte container header, SHA-256 `05147476fc70c91cc77ad7da6d6772376b2081d460ac7b1912e2d73868a09498`. OTA container bytes and raw flash offsets are different coordinate systems. The structural auditor checks checksum, declared lengths, headers/load addresses, placement, and known preserve ranges. It does not establish physical boot acceptance for a new image.

[Stock OTA requirements](stock-ota-requirements.md) separates the stock 1.1.15 request, HTTP, checksum and activation checks from our stricter image profile and the factory loader's boot requirements, with an evidence register.

The existing layout flags remain conservative profile assertions: incomplete map, no general verified-capacity guarantee, and no general boot-acceptance guarantee. Later observed read coverage and diagnostic boot apply to the research unit and exact image, rather than every image that satisfies the JSON layout.

SDK `plaf103` profiles overlay stock-derived application-bank addresses and startup pin pulls. They do not replace or validate the SDK's separate generated bootloader. Generated loader files are unsupported replacement outputs. See [build profiles](../development/firmware-builds.md) and [guarded recovery](../hardware/recovery.md).

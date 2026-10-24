# Experimental recovery reference

The retained conservative UART writers are deliberately pinned to the tested research-unit baseline and one reviewed diagnostic image. They are **not general PLAF103 installers**. A new backup or a freshly compiled diagnostic will normally fail their exact hash checks. Petlibre does not include those private binary inputs.

## Established and unresolved behavior

Physical research demonstrated independent loader entry, bounded body-sector write-back, marker activation with preservation checks, execution of a small stock-image Rust probe, and installation/boot of the UART-only synthetic diagnostic. The later source update records exact stock restoration and subsequent normal stock 1.1.15 boot, completing that conservative round trip. Full-bank restoration and power-cut recovery remain physically unverified.

Later diagnostics also added artifact-bound startup, physical STOP/off requests, and bounded indicator control while leaving the full feeding backend unopened. This does not validate motor operation, schedules, networking, watchdog timing, or sustained runtime reliability.

## Retained tools and gates

| Tools | Purpose |
| --- | --- |
| `petlibre-flash read`, `petlibre-flash decode-status`, `petlibre-audit backup` | Restricted inspection/read transport, protection decoding, and independent backup comparison |
| `petlibre-flash plan-stock-recovery`, `petlibre-flash stock-recovery` | Offline plan and explicit stock-bank transport; ordinary recovery refuses a still-bootable target bank |
| `petlibre-flash inspect-journal` | Journal verification |
| `petlibre-flash`'s emulated fixture tests (`petlibre-emu`) | The pinned helper on synthetic NOR: update, give-way, stock recovery and a mid-recovery fault; no hardware operation |

The physical stock round trip above was done with the Python clients (`uart-stock-recovery.py` and its modules), removed October 23, 2026. `petlibre-flash stock-recovery` keeps their plan, journal and wire formats. It restored bank 1 on the research unit on October 23, 2026 (FW38): a give-way first, then a full read, a reviewed plan and its execution; stock 1.1.15 booted from bank 1, and the reference build went back over UART.

Writers require source-bound plans, current-image evidence, explicit serial identity, immutable inputs, and full preimage verification. Sector writes have durable intent records and exact readback. Activation withholds the first eight marker bytes until staged image and preserved ranges match; the final marker write adds no erase. Lost ACKs or uncertain writes stop without automatic retry or resume.

Preserve current bytes outside the allowed application region, including unclassified bytes. Do not substitute blanket chip erases, protection clearing, arbitrary images, or disabled checks. Hash equality does not prove physical device identity.

## Using the reference

Read tool help without a device attached. Inspect [fixture requirements](../../development/fixtures.md) and the source restrictions before preparing any plan. Historical plans and captures cannot authorize a new bench session. Changing the profile requires a separately reviewed compatibility/recovery design and hardware acceptance, rather than editing a hash until the writer accepts an input.

No contributor check performs serial operations. No SDK build automatically flashes an image or starts the feeding owner.

A separate [UART development updater](../../development/uart-development.md) now accepts frozen reviewed source/target images within the tested bank layout. It verifies application regions and guard ranges rather than scanning all flash, and does not relax these conservative writers. Stock return through that newer updater remains untested.

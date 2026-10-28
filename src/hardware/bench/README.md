# Wellfed Bench board

The Wellfed Bench is an RP2040 USB-C board for recovering and developing on PETLIBRO feeders. It replaces the FT232R UART adapter and SH-UR04A relay board of the [relay bench](../../../docs/devices/plaf103/bench.md). Over one USB cable it shows up as a feeder UART bridge plus an `AT+CHn` relay port, so the bench tools can drive it the way they drive the relay board. This page covers the hardware: what is on the board, the files here, and how to order it. The firmware has [its own page](../../firmware/bench/README.md).

> [!NOTE]
> The board is designed but has never been built. Nothing here has been confirmed on hardware, and the bench tools do not use it yet.

> [!WARNING]
> Rev C is not ready to order. Its schematic adds two test pads, TP4 on `RUN` and TP5 on `QSPI_SS` (BOOT), that the board does not place or route yet: they sit outside the outline. Place and route them, run DRC, then export the gerbers.

## What it does

- **Feeder power.** A switched 5 V USB-A output (AP2171W, about 1.1 A limit) with an overcurrent flag, voltage and current sensing (INA181 on 50 mΩ).
- **Loader strap.** Holds feeder TX to GND through 4.7 kΩ, the ROM loader entry.
- **UART isolation.** A 74LVC2G125 buffer per direction, so the bridge cannot back-power a feeder that is off.
- **Aux power.** A second USB-C input, routed to the feeder by a TPS2116 when the host port cannot supply enough.
- **Feeder-alive sense.** Reads the feeder's TX level, so the board can tell that the D-cells still power a feeder whose 5 V is cut.
- **RECOVER button.** Power-cycles the feeder into its ROM loader without a host.
- **Room to grow.** 16 MB of flash (the firmware uses the first 2 MB), a 2×8 expansion header and an SWD header.

The relay channels, LEDs, buttons and pin map are listed in the [firmware page](../../firmware/bench/README.md#relay-port).

## Files

| Path | Contents |
| --- | --- |
| `kicad/` | The KiCad 10 project `wellfed_bench`: schematic, board and the `wellfed` symbol library |
| `fab/` | JLCPCB assembly files: the BOM and the placement file. The gerbers join them once rev C's test pads are placed |
| `enclosure/` | The OpenSCAD two-part enclosure and its button caps |

## Order from JLCPCB

1. Export the gerbers (F/B copper, paste, silkscreen and mask, Edge.Cuts) and the PTH and NPTH Excellon drill files from `kicad/wellfed_bench.kicad_pcb`, zip them, and upload the zip.
2. Turn on assembly, then upload `fab/wellfed_bench-BOM.csv` and `fab/wellfed_bench-CPL-jlcrot.csv`.
3. Set "Confirm Parts Placement" to Yes, and check every pin 1 and LED polarity in their review.

The `-jlcrot` placement file has JLCPCB's rotations and the J4 position corrected by hand. Regenerating it from KiCad loses those corrections, so edit it rather than replace it when parts move.

## Print the enclosure

The enclosure is two printed parts plus three button caps, held by four M3×12 screws into nuts pressed into the base. The assembly notes are at the top of the `.scad` file. Render each part to STL:

```sh
cd src/hardware/bench/enclosure
openscad -D 'part="base"' -o base.stl wellfed_bench_enclosure.scad
openscad -D 'part="lid"' -o lid.stl wellfed_bench_enclosure.scad
openscad -D 'part="button"' -o button.stl wellfed_bench_enclosure.scad
```

The STLs are not tracked. The base is sized for SLA resin, which cracks if screws self-tap into it.

## Edit the design

Open `kicad/wellfed_bench.kicad_pro` in KiCad 10. The board's logo is the one-color Wellfed lockup from [assets/brand](../../../assets/brand/README.md), flattened into hole-free polygons and mirrored on the back silkscreen.

After a board change, regenerate the gerbers and check the BOM and placement file against it before ordering.

## License

The board design, fab outputs and enclosure in this folder are licensed under the [CERN Open Hardware Licence v2, Permissive](LICENSE) (CERN-OHL-P). The firmware is MIT, like the rest of the repository.

# PLAF103 hardware

## Identification and evidence

The inspected research feeder is a PETLIBRO Granary **PLAF103**, 5 L, with FCC ID **2A3DEPLAF103S**. Board photographs identify **RTL8721DM** and **GD25Q64ESIG**. UART reports flash ID `C8 40 17`, consistent with the identified GD25Q64E's nominal 64 Mbit (**8 MiB**) capacity. Two separately acquired matching 8 MiB reads establish external-flash read coverage on that unit.

The [FCC internal-photo exhibit](https://fccid.io/2A3DEPLAF103S/Internal-Photos/Internal-Photos-6465982.pdf) is a comparison reference and shows a board marked `PLAF103 MB V1.0`; the actual research unit's PCB revision has not been established. The [GD25Q64E datasheet](https://download.gigadevice.com/Datasheet/DS-00484-GD25Q64E-Rev1.6.pdf) is the external-flash reference. These links identify research sources; compare markings on your own unit.

Evidence levels used here:

- **Observed:** labels/parts, UART output, or a recorded physical operation on the research unit.
- **Recovered:** behavior traced in stock 1.1.15 instructions, not electrically verified pin wiring.
- **Modeled:** host tests or execution with supplied register/ROM/NOR models.
- **Unknown:** unresolved board, electrical, timing, or mechanical behavior.

Do not assume PLAF108 compatibility. No schematic, complete connector pinout, measured motor driver behavior, or complete physical GPIO mapping is established.

## Recovered signal assignments

The table records stock 1.1.15 software roles. The SDK pin encoding is `(port << 5) | pin`. Electrical polarity, physical traces, and calibration still require board measurements.

| Signal | Stock role and initialization | Important unknown |
| --- | --- | --- |
| PA13 / PA14 | Motor output pair; initially both high | Driver braking/coasting and physical bowl/direction mapping |
| PA18 | Portion feedback input; both-edge interrupts during feeding | Mechanical phase and feedback timing |
| PA17 | Food-level input | Physical beam polarity and filter calibration |
| PA16 | Outlet input and passage latch | Physical obstruction polarity and timer activation |
| PA15 / PB3 | Optical enable outputs; initially high | Which line powers each emitter and duty limits |
| PA19 | External-power presence input | Electrical transition behavior |
| PB1 | ADC current estimate | Current scaling and trip thresholds |
| PB2 | ADC battery/power measurement | Divider scaling, calibration, and battery percentage |
| PA25 | Buzzer output; initially low | Real buzzer characteristics |
| PB5 / PB4 / PB7 / PB6 | Four button inputs | Printed-label mapping and physical debounce |
| PA2 / PA0 / PA4 / PB22 | Food warning / connection-OTA / lock / fault indicators | Only PB22 warning red is recorded; remaining colors unknown |

Useful stock instruction anchors: motor initialization `0x0e0091ac`, drive helpers `0x0e00960c` and `0x0e009634`, stop `0x0e01d868`, feedback ISR `0x0e01a55c`, sensor worker `0x0e011b58`, power worker `0x0e011f10`, and indicator worker `0x0e011738`.

## Recovered motor truth table

| Operation | Direction argument | PA13 | PA14 |
| --- | --- | --- | --- |
| Normal drive | 0 | 1 | 0 |
| Normal drive | Nonzero | 0 | 1 |
| Reverse recovery | 0 | 0 | 1 |
| Reverse recovery | Nonzero | 1 | 0 |
| Stock initialization/stop | Either | 1 | 1 |

Stock writes are sequential, and startup passes through input configuration. This table does not establish electrically safe transitions or clockwise/counterclockwise direction. Replacement hardware output support remains experimental.

See [UART wiring](uart.md), [boot/flash layout](../firmware/boot-and-flash.md), and [replacement architecture](../firmware/README.md).

## Later physical observations

After restoring the stock shared-pad supply prerequisite, a bounded diagnostic confirmed **PA0 Wi-Fi, PA2 low food, PA4 locked, PB22 warning**, HIGH-on/LOW-off on the tested unit. Each blinked twice and ended dark. PA0/PA2/PA4 input reads still disagree with visible HIGH output, and interleaved UART reports remain unresolved; these are not passing electrical-readback results. See [startup diagnostics](../firmware/startup-diagnostics.md).

CON3 carries board power as well as two motor leads traced to the gearbox. Unplugging it removed board boot/loader response, so it does not isolate the motor while preserving that power connection. Exact power pins and GPIO-to-driver routes remain unverified. Stopped-startup observations were made with the harness connected under supervision.

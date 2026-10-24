# PLAF103 board and signal map

What the PLAF103 main board connects to each [RTL8721DM](../../chips/rtl8721dm/README.md) pin, and what is known about each connection. Identification, chips and support status are on the [device page](README.md). Labels follow the [evidence levels](../../evidence.md).

## Recovered signal assignments

The table records stock 1.1.15 software roles. The SDK pin encoding is `(port << 5) | pin` ([chip](../../chips/rtl8721dm/README.md#pins)). Electrical polarity, physical traces, and calibration still require board measurements.

| Signal | Stock role and initialization | Important unknown |
| --- | --- | --- |
| PA13 / PA14 | Motor output pair; initially both high | Driver braking/coasting and physical bowl/direction mapping |
| PA18 | Portion feedback input; both-edge interrupts during feeding | Mechanical phase and feedback timing |
| PA17 | Food-level input; LOW when the beam is blocked (food present), HIGH when lit (FW13, physical) | How much food it takes to read present |
| PA16 | Outlet input and passage latch; LOW when blocked, HIGH when clear (FW13, physical) | Passage pulses with real food (FW16) and timer activation |
| PA15 / PB3 | Optical emitter enables, HIGH on: PA15 lights the food receiver, PB3 the outlet receiver (FW13, physical). PB3 is also SWD_CLK and needs SWD released first | Emitter current, duty limits and electrical OFF levels (FW30) |
| PA19 | External-power presence input; HIGH with the adapter, LOW on batteries (FW14, physical) | Its electrical levels and timing at the transition |
| PB1 | ADC current estimate | Current scaling and trip thresholds |
| PB2 | ADC battery/power measurement (channel 5) | Measured Oct 23: tracks the pack through a divider of about 2.06; battery percentage and low-voltage thresholds remain |
| PA25 | Buzzer output; initially low | Real buzzer characteristics; FW24 plays through the speaker instead |
| PB23 | Audio amplifier enable: stock holds it HIGH while the codec plays; FW24 does the same and the speaker plays (physical) | Electrical level and the amplifier part |
| PB4 / PB5 / PB6 / PB7 | Buttons, left to right: Reset, Mic, Dispense 1 portion, Lock. Active LOW with board pull-ups (high with the pad pulls off); no bounce seen at a 2 ms poll (FW20, physical) | Edge timing finer than 2 ms, and long-hold and simultaneous-press behavior |
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

See [UART wiring](uart.md), [relay bench control](bench.md), [boot/flash layout](flash-layout.md), and [replacement architecture](../../firmware/README.md).

## Later physical observations

After restoring the stock shared-pad supply prerequisite, a bounded diagnostic confirmed **PA0 Wi-Fi, PA2 low food, PA4 locked, PB22 warning**, HIGH-on/LOW-off on the tested unit. Each blinked twice and ended dark. PA0/PA2/PA4 input reads still disagree with visible HIGH output, and interleaved UART reports remain unresolved; these are not passing electrical-readback results. See [startup diagnostics](../../firmware/startup-diagnostics.md).

CON3 carries board power and the motor leads. The motor is the blue/white pair (a can motor with a suppression capacitor), driven by a TC118AH H-bridge beside CON3; the yellow pair is a 3.3 V logic line, most likely feedback. See [motor driver](motor-driver.md). Unplugging CON3 removed board boot/loader response, so it does not isolate the motor while preserving that power connection. The GPIO-to-driver routes remain untraced. Stopped-startup observations were made with the harness connected under supervision.

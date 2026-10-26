# Motor driver and FW30 electrical checks

Status: FW30 in progress. The ROM-loader and firmware STOP states are measured, and a startup drive pulse was found at the motor and fixed. The emitter OFF levels, the power-on flicker with the inputs probed, and resets remain.

## Driver

The motor driver is a **TC118AH** (Fine Made Microelectronics, SOP-8, single-channel H-bridge), designator U7, beside CON3. It was identified from the package marking (physical, October 21 2026); the datasheet is S&CIC2102 v1.1, kept in the ignored `private/references/`.

| Pin | Name | Pin | Name |
| --- | --- | --- | --- |
| 1 | NC | 8 | OUTA |
| 2 | INA | 7 | PGND |
| 3 | INB | 6 | AGND |
| 4 | VDD | 5 | OUTB |

| INA | INB | OUTA | OUTB | Mode |
| --- | --- | --- | --- | --- |
| L | L | Hi-Z | Hi-Z | Standby |
| H | L | H | L | Forward |
| L | H | L | H | Reverse |
| H | H | L | L | Brake |

Datasheet facts that matter here:
- **Pull-downs:** INA and INB have 150 kΩ internal pull-downs (120–200 kΩ). Undriven inputs therefore mean standby.
- **Logic levels:** VIH is at least 2.0 V and VIL at most 0.8 V, so 3.3 V GPIO drives it directly.
- **Supply:** VDD is 2.0–7.2 V.
- **STOP is brake:** stock STOP (PA13 = PA14 = HIGH), which the replacement copies, puts the driver in **brake**, with both outputs driven low.

The mapping of PA13 and PA14 to INA and INB is inferred from stock's complementary drive table. It is not traced. The two resistors marked `102` (1 kΩ) beside pins 2 and 3 look like series resistors from the SoC, but they are not traced either.

## Harness

On the research unit (physical, owner photographs):
- **Blue and white:** these go to a can-type DC motor with a disc suppression capacitor across its terminals. They are the motor leads, through the FB1/FB8 ferrite beads to OUTA/OUTB.
- **Yellow pair:** in the ROM loader it read 0 V and 3.3 V to ground, 3.3 V between them. That is a pulled-up logic line and its ground, most likely the PA18 feedback sensor, which read HIGH throughout FW13. Earlier notes calling these the motor leads were wrong.

## Measurements

Multimeter (Klein MM325), DC volts, COM on USB ground. The feeder was powered through the relay bench, with no batteries.

| State | INA (2) | INB (3) | VDD (4) | OUTA (8) | OUTB (5) | Motor |
| --- | --- | --- | --- | --- | --- | --- |
| ROM loader (`bench.py loader`, no application code) | 0.0 V | 0.0 V | 5 V | 0.13 V | 0.13 V | Still, no hum |
| Replacement firmware at STOP (FW13 review 02 running) | 3.3 V | 3.3 V | 5 V | 0 V | 0 V | Still; no hum at power-on |

In the ROM loader the driver is in **standby**: both inputs are held low by its pull-downs and both outputs float. That is the state every bench flash and dump runs in, so loader entry cannot drive the motor. This is the guarantee FW31 relies on.

Under the replacement firmware's STOP, both inputs are driven HIGH and both outputs are held at 0 V, which is the driver's **brake**. This is the first electrical confirmation that stopped-output startup reaches the driver, and that its STOP is brake, not drive. Both inputs are equal in this state, so it does not show which GPIO drives INA and which INB.

### Startup transients at the motor (October 24)

Logic analyzer at 2 MHz, `--probes outputs`: D2 and D3 on the legs of the motor's suppression capacitor (which join its two terminals), GND on the bench ground rail. Captures and reports are in the ignored `private/captures/fw30-outputs-*` and `private/reports/`.

| Session | Build | Cycles | After power-on |
| --- | --- | --- | --- |
| `check-01`, `check-02` | `feeder` before the fix | 4 boots | About 100 ms both HIGH (standby, outputs undriven), then **104.5–105 µs with D3 HIGH and D2 LOW** in every cycle, then both LOW (brake) |
| `check-03`, `boot-50` | `feeder` v154 (`e5b5c1dd…`) | 51 boots | 98–111 ms standby, then straight to brake. No differing read after the first millisecond in any cycle |
| `loader-50` | ROM loader | 50 entries | Standby (both HIGH) throughout |

**The 105 µs pulse was a real drive at every boot (physical; cause by static analysis).** Stopped startup preloaded both latches HIGH, then enabled the two pins as outputs one `gpio_init` call at a time, so PA13 drove HIGH while PA14 still read LOW through the driver's pull-down. The motor's winding joins the two leads, so one cannot sit at 5 V while the other is at 0 V unless the driver holds it there. Startup now enables both pins LOW (standby) and writes STOP to both in one port write (`stopped::enable_motor_stopped`); a host test fails on the old order. The pulse is absent from all 51 boots since.

**Power-on flicker (physical; not attributable to our code).** Within the first ~0.3 ms after the relay closes, the two outputs read differently for 0.5–11.5 µs in 39 of 50 boots, and for up to 110 µs in the loader, where no application code runs and the inputs read 0 V on a meter. The supply and the relay contacts are still settling then.

**Limit of outputs-only captures.** Before power-on, with the board unpowered and still discharging after the previous loader cycle (2 s off), the two leads read differently for up to 39 ms. They are joined by the winding and nothing powers the driver, so this is the analyzer's two channels resolving one common level near their threshold differently. A differing read is therefore proof of drive only when the levels are firm. The power-on flicker happens during that kind of ramp, so the outputs alone cannot call it drive or noise; the inputs, which are always driven or pulled down, can.

## Emitter test points

CON1 (silkscreen 余粮红外, "remaining-food infrared") carries the food-level sensor, with TP20 and TP21 beside it near Q5. CON2 carries the outlet sensor, with TP22 beside it. The `fw13-optics-v2` image (5 s windows) holds each emitter state long enough to read a meter.

| Test point | Emitters off | PA15 on | PB3 on | Both on |
| --- | --- | --- | --- | --- |
| TP20 | 3.3 V | 1.2 V | 3.3 V | 1.2 V |
| TP21 | ~2.68 V | ~2.68 V | ~2.68 V | ~2.68 V |
| TP22 | 0.66 V | 0.11 V | 0.66 V | 0.11 V |

TP20 follows only the food emitter (PA15). It reads 3.3 V when off and about 1.2 V, a typical infrared LED forward voltage, when on. It is on the food emitter's drive path, and that emitter's OFF state is 3.3 V at this node (physical, one run). TP21, held for a whole run, read 2.67–2.68 V on 13 of 16 cues regardless of emitter state. The three ~0 V readings did not recur with any state and are taken as probe slips. It is not an emitter drive or receiver output, most likely a supply or bias node. TP22, held for a whole run, matched the table on all 19 cues over three cycles. It follows PA15 only, not PB3, despite sitting beside CON2. The levels (0.66 V dark, a base-emitter clamp; 0.11 V lit) fit the food receiver's phototransistor driving an inverting stage, which would make PA17 HIGH when lit, as FW13 measured. That is inferred from voltages, not traced.

The food emitter's OFF state is therefore confirmed electrically: TP20 at 3.3 V, no LED current. No test point found so far follows PB3; the outlet sensor's own connector does. **CON2, physical (October 26, owner probing at the wire entries, feeder v232 at rest, then during 1-portion feeds 46 and 47 with the emitters on):** green 0 V (ground), blue 3.3 V (supply), white 2.6 V at rest and 0.03 V while feeding, red 0.6 V at rest and 0.08 V while feeding. White is the emitter's switched side: the driver pulls it to ground to light the LED, and at rest it sits at 2.6 V with no current path, so the outlet emitter's OFF state is confirmed electrically. Red behaves like TP22 on the food sensor (a receiver output, pulled low when lit). The roles are inferred from the levels (static); the levels are physical.

## Remaining for FW30

1. **Outlet emitter OFF level (PB3):** done (CON2 white wire, above). Emitter current for both emitters remains: the voltage across each emitter's series resistor with a meter, or a supply-current meter.
2. **Transients:** the boot pulse is fixed and the outputs are clean after the first millisecond ([above](#startup-transients-at-the-motor-october-24)). Left: the power-on flicker, with all four lines probed (micro IC hooks on pins 2, 3, 8 and 5), and resets. Use `--off-seconds 10` for loader sessions so the board has discharged before each capture starts. The procedure:
   1. Install `sigrok-cli` and the `fx2lafw` firmware (Arch: `sigrok-firmware-fx2lafw`); without it the clone enumerates but cannot open.
   2. Wire D0 to INA (pin 2), D1 to INB (pin 3), D2 to OUTA (pin 8), D3 to OUTB (pin 5), and analyzer GND to board GND. Check the analyzer's input rating before connecting the 5 V outputs.
   3. Run `wellfed-bench transients NAME --cycles 50` and `--mode loader`.
   4. Audit with `wellfed-audit transients --capture private/captures/NAME --output NEW-REPORT`.

   A pass needs, on every cycle: an unpowered start, no sample with INA ≠ INB or OUTA ≠ OUTB, and the brake reached and held after boot, or standby held throughout a loader entry.

   **Outputs only:** the driver's pins are 1.27 mm apart, too close for ordinary hooks. `--probes outputs` records only D2 and D3 at the motor (on its terminals, or on the suppression capacitor's legs where glue covers the tabs: the curved leg on the blue-lead side to D2, the straight leg to D3; which of them is OUTA is not traced, and the check does not depend on it), with analyzer GND on the bench ground rail. The motor turns only while those two differ, but near the analyzer's threshold one common level can read differently on the two channels ([above](#startup-transients-at-the-motor-october-24)), so a differing read during a supply ramp needs the inputs to settle. In standby the outputs are undriven and read at the analyzer's floating level (HIGH on the owner's clone, whose unconnected channels also read HIGH), so the audit does not claim the brake reached, or the power-on edge. The report records `probed: outputs` and what was not seen, and the polarity item stays open.
   **A reset mid-drive (open):** when a watchdog reset ended a drive (October 26, `fw16-stopfail-20261026-03`), the motor's leads read differently three more times, for 38–110 µs, within 0.2 ms of the drive's end. Two reads had the drive's polarity and one the opposite, and no drive followed (physical; [result](../../firmware/motor-pulse.md#result-october-26)). Back-EMF from the coasting motor would explain the first two but not the third, and outputs alone cannot tell. Check this transition with the inputs probed, beside the power-on flicker.
3. **Polarity:** confirm the stock-derived polarity electrically, by tracing PA13/PA14 to INA/INB.

Probe pins 2–4 at the tip of each foot. Pin 3 (INB) is next to pin 4 (VDD), and bridging them drives INB high, which with INA low is reverse drive.

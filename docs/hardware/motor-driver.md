# Motor driver and FW30 electrical checks

Status: FW30 in progress. The ROM-loader and firmware STOP states are measured. The emitter OFF levels, transients and repeated resets remain.

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

## Emitter test points

CON1 (silkscreen 余粮红外, "remaining-food infrared") carries the food-level sensor, with TP20 and TP21 beside it near Q5. CON2 carries the outlet sensor, with TP22 beside it. The `fw13-optics-v2` image (5 s windows) holds each emitter state long enough to read a meter.

| Test point | Emitters off | PA15 on | PB3 on | Both on |
| --- | --- | --- | --- | --- |
| TP20 | 3.3 V | 1.2 V | 3.3 V | 1.2 V |
| TP21 | ~2.68 V | ~2.68 V | ~2.68 V | ~2.68 V |
| TP22 | 0.66 V | 0.11 V | 0.66 V | 0.11 V |

TP20 follows only the food emitter (PA15). It reads 3.3 V when off and about 1.2 V, a typical infrared LED forward voltage, when on. It is on the food emitter's drive path, and that emitter's OFF state is 3.3 V at this node (physical, one run). TP21, held for a whole run, read 2.67–2.68 V on 13 of 16 cues regardless of emitter state. The three ~0 V readings did not recur with any state and are taken as probe slips. It is not an emitter drive or receiver output, most likely a supply or bias node. TP22, held for a whole run, matched the table on all 19 cues over three cycles. It follows PA15 only, not PB3, despite sitting beside CON2. The levels (0.66 V dark, a base-emitter clamp; 0.11 V lit) fit the food receiver's phototransistor driving an inverting stage, which would make PA17 HIGH when lit, as FW13 measured. That is inferred from voltages, not traced.

The food emitter's OFF state is therefore confirmed electrically: TP20 at 3.3 V, no LED current. No test point found so far follows PB3, so the outlet emitter's OFF level needs another probe point.

## Remaining for FW30

1. **Outlet emitter OFF level (PB3):** find a node that follows PB3 (the outlet sensor's connector pins or its own test points). Emitter current for both emitters (the FNB58 would give it from supply current).
2. **Transients:** across relay power-ons, resets and loader entries, between power-up and our stopped startup. A meter cannot catch these. Tooling is ready for the owner's 24 MHz fx2lafw analyzer:
   1. Install `sigrok-cli`.
   2. Wire D0 to INA (pin 2), D1 to INB (pin 3), D2 to OUTA (pin 8), D3 to OUTB (pin 5), and analyzer GND to board GND. Check the analyzer's input rating before connecting the 5 V outputs.
   3. Run `wellfed-bench transients NAME --cycles 50` and `--mode loader`.
   4. Audit with `wellfed-audit transients --capture private/captures/NAME --output NEW-REPORT`.

   A pass needs, on every cycle: an unpowered start, no sample with INA ≠ INB or OUTA ≠ OUTB, and the brake reached and held after boot, or standby held throughout a loader entry.
3. **Polarity:** confirm the stock-derived polarity electrically, by tracing PA13/PA14 to INA/INB.

Probe pins 2–4 at the tip of each foot. Pin 3 (INB) is next to pin 4 (VDD), and bridging them drives INB high, which with INA low is reverse drive.

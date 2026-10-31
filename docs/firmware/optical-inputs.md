# Optical inputs

The feeder watches its food with infrared beams. One beam looks across the hopper and says whether food is there; it sits above the rotor, so it is a low warning that reads low with about five portions left. Another looks across the outlet chute and says whether it is clear or blocked. A third input, the rotor's mark sensor, tells the feeder when a portion has turned past. This page covers how the firmware drives the emitters, reads the receivers and keeps both bounded, and the optics probe that proved them.

| Input | Pin | Emitter | Reads |
| --- | --- | --- | --- |
| Food beam | PA17 | PA15 | LOW is food present |
| Outlet beam | PA16 | PB3 | HIGH is clear, LOW is blocked |
| Mark sensor | PA18 | none | Rests HIGH, LOW at each rotor mark |
| Adapter | PA19 | none | HIGH while the adapter is in ([power](power.md)) |

A blocked beam reads LOW on both receivers, the same as with its emitter off. This matches stock. What the mark sensor means for portions is in [calibration](calibration.md#what-a-portion-is).

## How readings are qualified

Beam states come from the stock-derived `SensorInputs` filter. A reading needs 100 ms of optical settling, and a bad state needs 1 s of continuous bad readings. States are qualified only while both emitters are on.

While the emitters are on under supervision, the owner publishes the states, and health reports `outputs: "sensing"` instead of a stopped-readback fault. "Sensing" means motor STOP read back and the emitters exactly as published.

Each receiver changes within one 2 ms poll of its emitter turning on, and within 1–3 ms of it turning off. Stock's 100 ms settle is pure margin at that resolution.

Receiver levels show beam state only. They do not show beam geometry, or how much food it takes to read "present".

## PB3 is SWD_CLK

The outlet emitter, PB3, is also the SWD clock pin (`SWD_CLK` in the SDK pin map). `REG_SWD_PMUX_EN` bit 0 resets to 1, giving the pad to SWD. Stock's sensor GPIO setup calls the SDK's `sys_jtag_off()`, which clears that bit, immediately before it configures PB3.

Stopped-output startup and `Board::open_outputs` release SWD first and read the bit back. Stopped readback requires it clear, so an unreleased PB3 fails startup. The host fake keeps PB3 undriven until release, which reproduces the failure. Flashing uses UART, so losing SWD costs nothing.

## The optics probe

The optics probe (`src/sdk/wellfed-probes/src/optics.rs`) proves the emitters, receivers and the supervision that bounds them, with the motor stopped. It runs as its own KM4 task (priority 3), in the bring-up firmware's `optics` variant and as the [QA firmware](qa.md)'s first check. It:

1. Opens the board: motor STOP, emitters OFF, sensor inputs PA16–PA19.
2. Binds a fresh flash authority to the KM0 peer with the boot handshake ([bring-up probes](bring-up-probes.md#boot-handshake)), only after stopped-output readback, and attaches it to the board. No flash hook provider is installed in this image, so no lease can occur. The attachment satisfies the board's rule that an emitter needs an authority.
3. Opens the supervisor: TIM2 every 10 ms, owner lease 300 ms (the motor lease).
4. Runs the emitter windows of its plan (below). Each on window arms a non-renewable supervisor budget of its length plus 500 ms. Each 2 ms poll reads PA16–PA19 in one port access and renews the owner lease through `Events::observe`.
5. Ends with a deliberate owner stall: both emitters on, then the task spins for 1 s without renewing. The timer interrupt must cut them.

| Plan | Policy | Windows |
| --- | --- | --- |
| 1 | `fw13-optics-v1` | A 2 s baseline with both off, then six cycles of PA15 alone (2 s), PB3 alone (2 s) and both (4 s), each followed by 4 s off: 37 windows |
| 2 | `fw13-optics-v2` | Every window 5 s, three cycles, about 95 s, then the same stall |

The image runs plan 2, whose longer windows suit meter readings. Its start record announces it (`PLOPT1 start created=1 plan=2`), and the startup policy binds that string both ways. Each policy has its own schedule, so plan 1 evidence still audits.

### Records

`PLOPT1` lines:

- `open board= ready= attached= events= inputs= ms=`
- `window n= emitters=off|a|b|both start= len= samples= gap= edges= dropped= food=unknown|present|low outlet=unknown|clear|blocked stopped=`
- `inputs n= highs=food,outlet,feedback begin= end= first= low_min=`: high counts, levels at the window's start and end, ms to each input's first change, and the shortest complete LOW run
- `edge n= input= level= ms=`: up to 16 per window, the rest counted as dropped
- `stall cut= fault= off= stopped= closed=`, then `done windows=`
- `timeout ms=` or `error kind=supervisor|clock|capture|observe|optics n= fault= stall=`, which end the probe with emitters off. `stall=` is the milliseconds since the last good sample, or `-` without a clock. Past 300 ms, the task was starved for longer than the owner lease.

A window's records are logged at least 300 ms into the next window, so logging never delays settling samples.

A passing run has the planned sequence and timing, every gap within the filter's 100 ms limit, `stopped=1` throughout, a cut 300–340 ms after the last renewal with the OWNER fault, and the supervisor closed. An unattended run also needs the same inputs lit by each selection in every cycle. A blocking run needs the operator's blocking seen through the filter: food both present and empty, and the outlet both clear and blocked.

## Limits

- **Food passing the outlet.** Stock latches food passage through the outlet beam. The probe resolves LOW runs to about 2 ms, but a real pulse needs motion. Feeds now count outlet-beam breaks as a lower bound ([calibration](calibration.md#delivered-food-estimate)).
- **Emitter current, thermal duty and electrical OFF levels** are not measured. The probe's duty (40 % over two minutes, each window budgeted) is a bound, not a characterization.
- **Ambient light** on the receivers is not measured, and the owner does not yet check for a receiver lit with its emitter off ([jams and sensor faults](jams-and-sensor-faults.md#outlet-and-food-sensors)).

## Evidence

Observed on the research unit, October 21, 2026, over the UART. The hopper was empty, the motor stopped, and the unit ran on adapter power with no batteries. Both runs used plan 1 and passed the probe's auditor, which has since been removed; its reports remain.

- **Unattended run.**
  - Pairing: PA15 lights the food receiver (PA17), and PB3 lights the outlet receiver (PA16). All six cycles agreed.
  - Settling: within one 2 ms poll on, 1–3 ms off.
  - Empty hopper: the lit food beam read "low" through the filter, and the clear chute read "clear".
  - Mark sensor: PA18 read HIGH throughout, with emitters off and on. Neither emitter lights it.
  - Gaps: the longest gap between polls was 21 ms, with Wi-Fi and the transport running.
  - Stall: the cut came 305 ms after the last renewal, with fault 7 (OWNER). Emitters were off, the motor at STOP, the supervisor closed.
- **Blocking run.** The operator held paper across each beam on a timed script.
  - Outlet: the both-on window inside the block reported "blocked", and the windows around it "clear".
  - Food: the food line dropped when the block began, and the both-on window inside it reported "present".
  - Stall: the cut came 300 ms after the last renewal.
- **PB3.** Observed: the first bench run stopped when PB3 failed its enable readback. Our startup had not released SWD, so the latch never reached the pad, which read its pull-down. Every earlier "PB3 off" readback reflected the pull-down, not a driven LOW. PB3's OFF level now comes from a driven pin.
- **Food beam with food.** Observed October 29, owner weighing: a feed with food in read "present". From a hand-emptied hopper, eight 1-portion feeds read "low" from their start, yet dropped 36.8 g, about five portions at 7.11 g ([calibration](calibration.md#evidence)).
- **Mark sensor.** Observed later, with motion: PA18 is the rotor's mark sensor ([motor pulse](motor-pulse.md#evidence)).
- **Optics fault on trial boots.** Observed: the first WPA3 join starved the probe past its owner lease (`stall=703`). The radio now waits for startup ([QA](qa.md#known-issues)).

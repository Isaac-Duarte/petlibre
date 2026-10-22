# FW13 food, outlet and feedback inputs

Status: Done (motion-free scope). Emitter/receiver pairing, blocked/clear polarity, settling, bounded emitter windows and the owner-stall cut-off passed on the research unit. Food passing the outlet beam, and the feedback input, need motion and are carried to FW16.

## Probe

`probes/optics.rs` runs as its own KM4 task (priority 3) after the LED probe, beside the FW07 Wi-Fi probe and the FW08 transport. It:

1. Opens the board: motor STOP, emitters OFF, sensor inputs PA16–PA19.
2. Binds a fresh flash authority to the KM0 peer (the FW06 HELLO/READY handshake), only after stopped-output readback, and attaches it to the board. No flash hook provider is installed in this image, so no lease can occur. The attachment satisfies the board's rule that an emitter needs an authority.
3. Opens the supervisor: TIM2 every 10 ms, owner lease 300 ms (the motor lease).
4. Runs 37 windows. The first is a 2 s baseline with both emitters off. Then come six cycles of PA15 alone (2 s), PB3 alone (2 s) and both (4 s), each followed by 4 s off. Each on window arms a non-renewable supervisor budget of its length plus 500 ms. Each 2 ms poll reads PA16–PA19 in one port access and renews the owner lease through `Events::observe`.
5. Ends with a deliberate owner stall: both emitters on, then the task spins for 1 s without renewing. The timer interrupt must cut them.

Food and outlet states come from the stock-derived `SensorInputs` filter: 100 ms optical settling, and 1 s of continuous bad readings before a bad state. They are qualified only while both emitters are on. While emitters are on under supervision the probe publishes them, and health reports `outputs: "sensing"` (motor STOP read back, emitters exactly as published) instead of a stopped-readback fault.

Records (`PLOPT1`, policy `fw13-optics-v1`, bound both ways to `PLOPT1 start created=`):

- `open board= ready= attached= events= inputs= ms=`
- `window n= emitters=off|a|b|both start= len= samples= gap= edges= dropped= food=unknown|present|empty outlet=unknown|clear|blocked stopped=`
- `inputs n= highs=food,outlet,feedback begin= end= first= low_min=`: high counts, levels at the window start and end, ms to each input's first change, and the shortest complete LOW run
- `edge n= input= level= ms=`: up to 16 per window, the rest counted as dropped
- `stall cut= fault= off= stopped= closed=`, then `done windows=`
- `timeout ms=` or `error kind=supervisor|clock|capture|observe|optics n= fault=` end the probe with emitters off

A window's records are logged at least 300 ms into the next window, so logging never delays settling samples. `tools/audit-firmware-optics.py --stage b|c` checks them. It requires the sequence and timing above, every gap within the filter's 100 ms limit, `stopped=1` throughout, a cut 300–340 ms after the last renewal with the OWNER fault, and the supervisor closed. Stage b (unattended) also requires the same inputs lit by each selection in every cycle. Stage c requires the operator's blocking seen through the filter: food both present and empty, and the outlet both clear and blocked.

## PB3 is SWD_CLK

The first bench run stopped when PB3 failed its enable readback. PB3 is the SWD clock pin (`SWD_CLK` in the SDK pin map), and `REG_SWD_PMUX_EN` bit 0 resets to 1, giving the pad to SWD. Stock's sensor GPIO setup calls the SDK's `sys_jtag_off()` (clear that bit) immediately before it configures PB3. Our startup did not, so the latch never reached the pad, which read its pull-down. Every earlier "PB3 off" readback (FW03 onward) therefore reflected the pull-down, not a driven LOW.

Stopped-output startup and `Board::open_outputs` now release SWD first and read it back. Stopped readback requires the bit clear, so an unreleased PB3 fails startup. The host fake keeps PB3 undriven until release, which reproduces the failure. Flashing uses UART, so losing SWD costs nothing.

## Bench results

Physical, research unit, October 21 2026. The hopper was empty, the motor stopped, and the unit ran on adapter power with no batteries.

**Unattended run (stage b, passed):**

- **Pairing:** PA15 lights the food receiver (PA17). PB3 lights the outlet receiver (PA16). All six cycles agreed.
- **Settling:** each receiver changed within one 2 ms poll of its emitter turning on, and within 1–3 ms of it turning off. Stock's 100 ms settle is pure margin at this resolution.
- **Empty hopper:** the lit food beam read "empty" through the filter, and the clear chute read "clear".
- **Feedback:** PA18 read HIGH throughout, with emitters off and on. Neither emitter lights it.
- **Gaps:** the longest gap between polls was 21 ms, with Wi-Fi and the transport running.
- **Stall:** the cut came 305 ms after the last renewal, with fault 7 (OWNER). Emitters were off, the motor at STOP, and the supervisor closed.

**Blocking run (stage c, passed):** the operator held paper across each beam on a timed script.

- **Outlet:** the both-on window inside the outlet block reported "blocked", and the windows around it "clear".
- **Food:** the food line dropped when the block began, and the both-on window inside the block reported "present".
- **Polarity:** a blocked beam reads LOW on both receivers, the same as with the emitters off. This matches stock: food LOW is "food present", outlet HIGH is "clear".
- **Stall:** the cut came 300 ms after the last renewal.

## Limits and carried items

- **Food passage pulses** through the outlet beam (stock's passage latch) need motion. No real pulse was measured. The probe resolves LOW runs to about 2 ms. Carried to FW16.
- **PA18 feedback** needs the mechanism to move. Carried to FW16.
- **Emitter current, thermal duty and electrical OFF levels** are not measured. The probe's duty (40 % over two minutes, each window budgeted) is a bound, not a characterization. PB3's OFF level now comes from a driven pin. Both are carried to FW30.
- **Levels only:** receiver levels show beam state, not beam geometry, or how much food it takes to read "present".

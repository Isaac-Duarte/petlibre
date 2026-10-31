# Indicator lights

The panel has four single-color lights, one behind each icon. The indicator service decides what each light shows when several things want it at once, and it checks every change it makes. If a light cannot be trusted, the service turns them all off rather than show a wrong state.

Status: **Works**. The service is built and host-tested, and its colors and transitions passed the [bench check](#bench-check) with the owner watching (October 28). Which states show is still provisional.

## The lights

Seen from the front, the icons run low food, lock, Wi-Fi, error/warning from left to right. Each light sits on its own pad and is lit HIGH.

| Light (`wellfed_core::indicator::Light`) | Icon | Color | Pad | Input readback |
| --- | --- | --- | --- | --- |
| `connection` | Wi-Fi | White | PA0 | None |
| `food` | Low food | Red | PA2 | None |
| `lock` | Lock | White | PA4 | None |
| `fault` | Error/warning | Red | PB22 | Yes |

Port A's low pads read LOW whatever is driven on our unit, so only PB22 can be read back ([startup](startup-diagnostics.md#shared-pad-power)). The pads come from `wellfed_board_plaf103::pins` ([device layer](device-layer.md)).

## How the service decides

`wellfed_core::indicator` is `no_std` and allocation-free, and keeps no clock or task of its own.

- **Requests.** A request names a light, a pattern (`Off`, `Steady`, or `Blink { on_ms, off_ms }`), a priority, optionally a duration, and the requester's own label. `Indicators::show` returns a `Handle`, and `cancel` takes it.
- **Who wins.** Per light, the highest priority wins, and between equal priorities the newer request. The rest wait underneath, so a cancelled or expired winner reveals the next. An `Off` request holds a light dark over lower ones.
- **Fixed table.** `Indicators<S, SLOTS>` is an array, and a full table refuses (`Refused::Full`). A blink phase under 50 ms (`MIN_PHASE_MS`) is refused, and so is a duration of 0 or half the clock's range.
- **Handles** carry a 64-bit sequence number that never repeats in a boot. A handle kept past its request cancels nothing, even after its slot is reused.
- **Frames.** The loop calls `frame(now)`. Expired requests end, and the loop gets each light's level and the milliseconds until the next change (a blink edge or an expiry). Blinks start lit and are re-anchored at whole periods, so their phase survives the millisecond clock's wrap.
- **Claims.** `Claim` holds one request while a condition lasts. It shows the request when the condition starts, keeps it unchanged (same handle and phase) while it holds, cancels it when it ends, and replaces it when what it wants changes.
- **Driver.** `Drive` writes a light only when its level changes, waits the board's settle time (10 ms), then checks it (`Lights::check`). A refused write, or a level that does not hold, turns every light off, and the driver stops for the rest of the boot. The lock, feeding and the device API are unaffected.

## Never in a feed's way

The service runs in the loop task, so it adds no task: the product firmware uses two of the four a firmware gets. A step is a few comparisons over at most a dozen slots. For each light that changes, it is one masked port write under the interrupt lock and, a settle later, one readback. It takes no motor lease and no flash lease.

The board's indicator writes (`wellfed_hal::leds::show`, checked by `leds::shown`) do not require stopped outputs, unlike the bring-up probe's `leds::request`. A feed runs the motor and the emitters, and the lights must neither freeze nor fail then. What keeps that safe:

- The write is one masked write of indicator bits only, under the interrupt lock, so it cannot interleave with the supervision timer's motor writes.
- A compile-time assertion keeps the indicator masks disjoint from the motor and emitter masks.
- A host test drives every light during a simulated feed and checks that no motor or emitter bit moves.

The checks of latch, pad, direction and (on PB22) input stay as they were.

The check runs during feeds, up to once per blink edge per light, so it holds the interrupt lock only for what it needs. That is five register reads: both ports' directions, the latch, one input sample and the pad. It takes no stopped-output snapshot and none of the probe's 64 extra input samples. The write holds the lock for the direction reads and one port write. That is the longest the supervision timer can wait (static analysis, not measured; each read is an SDK or ROM call of a few instructions).

If a steady light drifts after it was set, the service notices only at the light's next change, and only some drifts:

- A lost direction refuses the next write, and a reconfigured pad fails the next check. Either way the lights go dark.
- A flipped latch is simply overwritten by the next write, which then checks clean, so it is never reported.

Startup still enables the indicators only from verified stopped outputs and leaves them LOW.

## What the feeder shows

`wellfed_feeder::feeder::lights` maps the feeder's states to the lights. What the settings hide follows stock: with `lights` off, or outside the lights window on the local clock, every light is dark, faults included, and the requests still hold so the lights come back as they were. While the clock is invalid the window is open. Which states show and how is still a provisional mapping that exercises the service on the board:

| State | When | Light | Pattern | Priority |
| --- | --- | --- | --- | --- |
| `fault` | The feeding owner holds a fault, or its journal failed | fault | Steady | 90 |
| `updating` | An install runs | connection | Blink 150/150 ms | 80 |
| `refused` | A Dispense press was refused (3 s) | fault | Blink 150/150 ms | 70 |
| `setup` | BLE setup mode runs | connection | Blink 500/500 ms | 70 |
| `pairing` | An offer waits for a Dispense press or its code | lock | Blink 250/250 ms | 70 |
| `battery` | The battery holds feeds | fault | Blink 1000/1000 ms | 60 |
| `feeding` | A feed is queued or runs | food | Blink 500/500 ms | 50 |
| `low-food` | The last feed's food beam read low | food | Steady | 10 |
| `locked` | The panel is locked | lock | Steady | 10 |
| `online` | The feeder has an address | connection | Steady | 10 |

With `lights` off every light is dark, as stock drives them all LOW. The requests still hold, so the lights come back as they were. Low food shows the last feed's reading, since the food beam is lit only during feeds: it stays on after a refill until a feed reads food again, and stays dark until this boot's first feed has read. Missing-time indications wait for the light policy features ([product architecture](product-architecture.md)).

Each pass, the loop steps the lights after readiness. A change in what a light shows is logged once, not at each blink edge: `PLIND1 show light=lock state=pairing`, or `state=none` when nothing does. A failure is `PLIND1 fail light=… check=refused|input-mismatch|abort`.

## Bench check

Run this with the owner watching. Nothing here moves the motor except step 7, which follows the bench rules: hopper empty, D cells out, at most 1 portion. Record the answers with the capture.

1. Install a `feeder` build with this service through the reviewed update path, and confirm it. Keep a UART capture (or `wellfed logs --follow`) running for the `PLIND1` lines.
2. **Boot.** You should see `show light=lock state=locked` and, once joined, `show light=connection state=online`. The lock and Wi-Fi icons are lit steady; food and warning are dark. Note each lit icon's color.
3. **Lock.** Run `wellfed lock off`, then `wellfed lock on`. The lock icon goes dark, then lights again, and the log shows `state=none`, then `state=locked`.
4. **Lights setting.** Run `wellfed settings --lights off`. All four icons go dark. Run `wellfed settings --lights on`, and lock and Wi-Fi return.
5. **Pairing offer.** Unlock (`wellfed lock off`), press Lock first and then add Dispense, and hold both to open the recovery window. Dispense pressed alone, even a moment early, starts a feed on an unlocked panel. Then start `wellfed recover --address …`. The lock icon blinks about twice a second until the code is keyed in or the offer times out, then returns to its state.
6. **Setup mode.** On an unlocked panel, hold Lock and Mic. The Wi-Fi icon blinks once a second until setup mode's reboot, which comes only after 15 minutes with no phone; `wellfed reboot` ends it sooner.
7. **Feed.** Run `wellfed feed --portions 1`. The food icon blinks once a second while the feed runs, and goes dark after its result. Check the feed's result and trace as usual: the lights must not change either.
8. **Refused press.** With a feed refused, press Dispense on an unlocked panel. The simplest case: press Dispense once to start a feed, then again about 3 s later while it runs. The second press is refused `busy`, and the warning icon flashes quickly for about 3 s.
9. **Steady fault.** With the `sensor-fault` variant installed ([jams and sensor faults](jams-and-sensor-faults.md#fault-injection-variants)), a 3-portion feed holds a fault. The warning icon is lit steady and red.

Record which steps passed, each icon's color, and any `PLIND1 fail` line. Watch especially for `fail light=fault`: PB22 is the one pad with input readback, so it is the only light whose check can catch an input mismatch. A `fail` line means the lights stayed off for that boot. That is the designed response, not a pass.

## Evidence

- **The bench check.** Observed (October 28, owner watching, `feeder` `f1e4655c…` on bank 1, then the `sensor-fault` variant for step 9): every step passed, with no `PLIND1 fail` line. Lock and Wi-Fi lit steady at boot; the lock went dark on unlock and back on lock; the `lights` setting darkened all four and restored them; the lock blinked through a recovery offer that timed out; the Wi-Fi light blinked in setup mode; low food blinked through each 1-portion feed and went dark after its result; a second Dispense press during a feed was refused `busy` and flashed the warning for 3 s; a held sensor fault lit the warning steady, and a later Dispense press changed nothing. Colors: low food and warning red, lock and Wi-Fi white.
- **Pins and polarity.** Observed: each light on its pad, lit HIGH, from the startup indicator probe with the operator watching and the owner's observation of the icon order (October 24). PB22 is red.
- **The service.** Modeled (October 26):
  - `wellfed-core/tests/indicator.rs`: priority, ties, cancellation and stale handles, expiry including across the clock's wrap, blink edges and phase across the wrap, refusals, claims, and the driver's settle-and-check and failure.
  - `wellfed-hal/tests/leds.rs`: the service's writes with the motor and an emitter on, and latch and direction failures.
  - `wellfed-fw-feeder/tests/lights.rs`: the mapping on the board's platform against the fake GPIO. Each state lights its pad, a fault shows over the battery blink and an offer over the lock, the refused flash ends after 3 s, the setting works, food blinks during a simulated feed with the motor bits untouched, a failure darkens all four for the boot, and a loop pass lights the lock and a held fault.
- **Builds.** Fresh `device` and `firstboot` `feeder` builds pass their link, RAM-park and stack audits (loop stack 5,412 of 8,192 B).
- **On the board.** Not yet: the bench check above needs the owner watching.

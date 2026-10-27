# Indicator service

The panel has four single-color lights, one behind each icon. Seen from the front they run low food, lock, Wi-Fi, error/warning from left to right. Each sits on its own pad and is lit HIGH (physical, FW04 and owner observation, October 24):

| Light (`wellfed_core::indicator::Light`) | Icon | Pad | Input readback |
| --- | --- | --- | --- |
| `connection` | Wi-Fi | PA0 | none (Port A reads LOW on the tested unit) |
| `food` | low food | PA2 | none |
| `lock` | lock | PA4 | none |
| `fault` | error/warning (red) | PB22 | yes |

Only PB22's color (red) is recorded; the other three colors are for the owner's bench check below. The pads come from `wellfed_board_plaf103::pins` ([device layer](device-layer.md)).

## The service

`wellfed_core::indicator` is `no_std`, allocation-free and keeps no clock or task of its own.

- **Requests.** A request names a light, a pattern (`Off`, `Steady`, `Blink { on_ms, off_ms }`), a priority and optionally how long it lasts, plus the requester's own label. `Indicators::show` returns a `Handle`; `cancel` takes it. Per light the highest priority wins, and between equal priorities the newer request; the rest wait underneath, so a cancelled or expired winner reveals the next. An `Off` request holds a light dark over lower ones.
- **Fixed table.** `Indicators<S, SLOTS>` is an array; a full table refuses (`Refused::Full`). A blink phase under 50 ms (`MIN_PHASE_MS`) or a duration that is 0 or half the clock's range is refused too.
- **Handles** carry a 64-bit sequence number that never repeats in a boot, so a handle kept past its request cancels nothing, even after the slot is reused.
- **Frames.** The loop calls `frame(now)`: expired requests end, and it gets each light's level and the milliseconds until the next change (a blink edge or an expiry). Blinks start lit and are re-anchored at whole periods, so their phase survives the millisecond clock's wrap.
- **Claims.** `Claim` holds one request while a condition lasts: shown when it starts, kept unchanged (same handle and phase) while it holds, cancelled when it ends, replaced when what it wants changes.
- **Driver.** `Drive` writes a light only when its level changes, waits the board's settle time (10 ms), then checks it (`Lights::check`). A refused write or a level that does not hold turns every light off, and the driver stops for the rest of the boot: a light that cannot be trusted shows nothing rather than a wrong state. The lock, feeding and the device API are unaffected.

### Never in a feed's way

The service runs in the loop task, so it adds no FreeRTOS task (the firmware's limit is two). A step is a few comparisons over at most a dozen slots and, per light that changes, one masked port write under the interrupt lock and, a settle later, one readback. It takes no motor lease and no flash lease.

The board's indicator writes (`wellfed_hal::leds::show`, checked by `leds::shown`) do not require stopped outputs, unlike the bring-up probe's `leds::request`: a feed runs the motor and the emitters, and the lights must neither freeze nor fail then. What keeps that safe is that the write is one masked write of indicator bits only, under the interrupt lock, so it cannot interleave with the supervision timer's motor writes; a compile-time assertion keeps the indicator masks disjoint from the motor and emitter masks, and a host test checks that no motor or emitter bit moves while every light is driven during a simulated feed. The checks of latch, pad, direction and (PB22) input stay as they were.

The check runs during feeds, up to once per blink edge per light, so it holds the interrupt lock only for what it needs: five register reads (both ports' directions, the latch, one input sample, the pad), with no stopped-output snapshot and none of the probe's 64 extra input samples. The write holds it for the direction reads and one port write. That is the worst case the supervision timer waits (static analysis, not measured; each read is an SDK or ROM call of a few instructions).

A steady light is checked only when it changes: a level that drifts afterwards (a pad reconfigured, a latch flipped) goes unnoticed until the light's next change, when its check fails and the lights go dark. Startup still enables the indicators only from verified stopped outputs and leaves them LOW.

## The feeder's mapping (provisional)

Which states show, how, and what the `lights` setting hides are FP12's policy, still to be settled against stock ([feature board](../../FEATURE_BOARD.md)). Until then `wellfed_feeder::feeder::lights` shows enough to exercise the service on the board:

| State | When | Light | Pattern | Priority |
| --- | --- | --- | --- | --- |
| `fault` | the feeding owner holds a fault, or its journal failed | fault | steady | 90 |
| `updating` | an install runs | connection | blink 150/150 ms | 80 |
| `refused` | a Dispense press was refused (3 s) | fault | blink 150/150 ms | 70 |
| `setup` | BLE setup mode runs | connection | blink 500/500 ms | 70 |
| `pairing` | an offer waits for a Dispense press or its code | lock | blink 250/250 ms | 70 |
| `battery` | the battery holds feeds | fault | blink 1000/1000 ms | 60 |
| `feeding` | a feed is queued or runs | food | blink 500/500 ms | 50 |
| `locked` | the panel is locked | lock | steady | 10 |
| `online` | the feeder has an address | connection | steady | 10 |

With `lights` off every light is dark, as stock drives them all LOW; the requests still hold, so the lights come back as they were. The low-food and missing-time indications ([product architecture](product-architecture.md)) wait for FP07 and FP12.

Each pass the loop steps the lights after readiness. A change of what a light shows is logged once (`PLIND1 show light=lock state=pairing`, `state=none` when nothing does), not each blink edge; a failure is `PLIND1 fail light=… check=refused|input-mismatch|abort`.

## Evidence

- **Host (simulation):** `wellfed-core/tests/indicator.rs` (priority, ties, cancellation and stale handles, expiry including across the clock's wrap, blink edges and phase across the wrap, refusals, claims, the driver's settle-and-check and failure); `wellfed-hal/tests/leds.rs` (the service's writes with the motor and an emitter on, latch and direction failures); `wellfed-fw-feeder/tests/lights.rs` (the mapping on the board's platform against the fake GPIO: each state on its pad, a fault over the battery blink, an offer over the lock, the refused flash ending after 3 s, the setting, a food blink during a simulated feed with the motor bits untouched, a failure darkening all four for the boot, and a loop pass lighting the lock and a held fault).
- **Physical:** not yet. The bench check below needs the owner watching.

## Bench check (owner watching)

Hardware first, then this check; nothing here moves the motor except step 7, which follows the bench rules (hopper empty, D cells out, at most 1 portion). Record the answers with the capture.

1. Install a `feeder` build with this service through the reviewed update path, and confirm it. Keep a UART capture (or `wellfed logs --follow`) running for the `PLIND1` lines.
2. **Boot.** Expect `show light=lock state=locked` and, once joined, `show light=connection state=online`. Owner: the lock and Wi-Fi icons are lit steady; food and warning are dark. Note each lit icon's color.
3. **Lock.** `wellfed lock off`, then `wellfed lock on`. Owner: the lock icon goes dark, then lights again; the log shows `state=none`, then `state=locked`.
4. **Lights setting.** `wellfed settings --lights off`. Owner: all four icons dark. `wellfed settings --lights on`: lock and Wi-Fi return.
5. **Pairing offer.** Unlock (`wellfed lock off`), hold Lock and Dispense to open the recovery window, then start `wellfed recover --address …`. Owner: the lock icon blinks about twice a second until the code is keyed in or the offer times out, then returns to its state.
6. **Setup mode.** On an unlocked panel, hold Lock and Mic. Owner: the Wi-Fi icon blinks once a second until setup mode's reboot.
7. **Feed.** `wellfed feed --portions 1`. Owner: the food icon blinks once a second while the feed runs and goes dark after its result. Check the feed's result and trace as usual: the lights must not change either.
8. **Refused press.** With a feed refused (for example during an install, or with the battery hold), press Dispense on an unlocked panel. Owner: the warning icon flashes quickly for about 3 s.
9. **Warning color and steady fault** come with FW19's `sensor-fault` variant on the bench (its third feed holds a fault): owner, the warning icon is lit steady and red.

Record which steps passed, each icon's color, and any `PLIND1 fail` line; watch especially for `fail light=fault`, since PB22 is the one pad with input readback and the only light whose check can catch an input mismatch. A `fail` line means the lights stayed off for that boot, which is the designed response, not a pass.

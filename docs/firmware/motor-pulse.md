# FW16 first supervised motor pulse

Status: run once on hardware, October 24 (owner present; [result](#first-run-october-24)). The owner chose to go ahead before FW30's power-on flicker is classified, since it also appears in the ROM loader.

## What the image does

`wellfed-build build --profile firstboot --firmware bringup --variant pulse` builds the bring-up firmware's `pulse` variant. It records `startup_motion_policy: fw16-pulse-v1` (declared in `src/firmware/wellfed/bringup/Cargo.toml`) and `variant: pulse` in the manifest and gets its own build ID. The bring-up firmware's normal image adds no task, its other variants start no motor task, and the builder refuses a motion policy outside a variant, so nothing else can drive the motor. The bench never builds it.

After the LED probe, the pulse probe (`src/sdk/wellfed-probes/src/pulse.rs`):
1. Skips everything unless this is a clean cold boot: reset reason 0 and its backup-register marker clear.
2. Opens the board (motor STOP, emitters OFF, sensors), binds a fresh flash authority to the KM0 peer and opens the supervisor (TIM2 at 10 ms, owner and motor lease 200 ms). It enables feedback capture.
3. Logs `armed`, then waits 60 s, renewing its owner lease every 2 ms, so the operator can be ready.
4. Sets the marker, then drives once: `DRIVE_0` (PA13 high, PA14 low) for 100 ms, then STOP. It counts PA18 feedback edges and samples motor current on ADC channel 4 every 10 ms.
5. Watches for 1 s more, then verifies STOP. It checks the gate state, the motor pins, the supervisor closing and stopped-output readback.

Bounds that hold even if the task stalls: the motor gate's 200 ms command lease and the owner lease, both enforced by the timer interrupt. A host test stalls the task mid-pulse and requires STOP within the lease. A drive is only applied through `Events::motor`, which needs a running timer, a live owner and enabled capture.

**Every clean cold boot pulses once.** The marker survives a watchdog reset but not power loss. The image must not stay installed after the session.

Records (`PLMOT1`):
- `start created=`
- `open board= ready= attached= events= capture= ms=`
- `armed delay_ms= pulse_ms= lease_ms=`
- `pulse dir= applied_ms= on_ms= edges= first_edge_ms= current=min,max,mean samples=`
- `after ms= edges= current=… stopped=`
- `done`
- `skip`, `timeout` or `error stage= fault= stopped=` end the probe stopped.

## Guards

- **`wellfed-bench`:** `flash-prepare` and `flash-execute` refuse a target whose manifest names a motion policy unless `--attended-motion` is given.
- **Startup audit:** binds `fw16-pulse-v1` to the `PLMOT1 start created=` format both ways, and refuses motion and optics policies together.
- **Prerequisites:** the board makes FW30 (transients) and FW14 (current channel) prerequisites. Do not install this before both pass.

## Procedure (owner present)

1. FW30 transients pass (`wellfed-bench transients`), and FW14 confirms the current channel reads apart from the battery channel.
2. Hopper empty, the outlet clear, hands away from the mechanism. The owner can cut USB power at any moment.
3. Install the frozen review with `--attended-motion` and `--boot-seconds` short (10). Power off right after the install.
4. Load a dashboard script anchored on `PLMOT1 armed`, with a countdown to the pulse at 60 s.
5. Cold boot with a 120 s capture. Observe the motion and record its direction and size.
6. Reinstall a non-motion image before anything else.

Pass: one `pulse` record with `on_ms` about 100 and `stopped=1` after it, the motor visibly moving only once, and no `error`. The `edges` and current values are the first real measurements of feedback and drive current. Direction 0's physical rotation and its INA/INB mapping are recorded from observation.

## First run (October 24)

Physical, owner present, hopper empty. `motor` `pulse` (review `fw16-pulse-01`, image `073940ec…`) went into bank 1 over UART with `--attended-motion` and a 10 s boot, then power was cut before the 60 s delay. The pulse boot (`fw16-pulse-01-run`) recorded:

```
PLMOT1 armed delay_ms=60000 pulse_ms=100 lease_ms=200
PLMOT1 pulse dir=1 applied_ms=0 on_ms=106 edges=0 first_edge_ms=none current=64,447,147 samples=10
PLMOT1 after ms=1000 edges=0 current=15,16,15 stopped=1
PLMOT1 done
```

- **Pass:** one pulse of 106 ms (the 10 ms timer tick), `stopped=1`, no error. The owner heard the motor; the movement itself was not seen.
- **At the motor** (logic analyzer on the two leads, 1 MHz): the only drive in 130 s was 106.07 ms with the white-lead side HIGH, 60.5 s after power-on. `dir=1` is `MotorCommand::Drive0`, PA13 HIGH and PA14 LOW. The old 105 µs boot pulse (PA13 alone) had the same polarity. Opening the board before arming showed a 212 µs coast (both inputs LOW, no drive).
- **Motor current (ADC channel 4, raw):** 64–447, mean 147 over 10 samples while driving; 15–16 stopped. First loaded readings; scaling is FW14's and FW19's.
- **Feedback:** no PA18 edge in the pulse or the second after it. Either 100 ms turns too little to pass a mark, or PA18 is not the rotation sensor.
- **Afterwards:** `feeder` v154 went back into bank 1 over UART (`fw16-restore-01`, region `d2357eb1…`) and booted at STOP.

Open: the feedback input. The direction was seen in the first owner feed: [clockwise](#first-feed-october-25).

## Not covered

Lease expiry with a stalled owner was host-tested and FW05-measured, but not under real motion; this session gave the first real-motion data, and the `feed-stall` variant tests the lease under motion ([below](#a-stalled-loop-and-a-failed-stop-during-a-feed)). Cancellation, failed stop, the journal-reserved feed, portions and reverse recovery belong to the full FW16 owner path and FW19. That owner runs inside `feeder`'s loop, and since v164 the device API feeds through it, so later motor tests run over the air: see [feeds through the owner](#feeds-through-the-owner).

## Feeds through the owner

Since v164 (`469935b4…`, October 25) `wellfed feed --portions N` runs a feed through `feeder`'s feeding owner: the journal reservation, the supervisor's 200 ms motor lease, the feed limits in `feeding.rs`, the result in the journal, and a `Fed` event. Nothing else moves the motor: the image has no startup motion and the bench never sends a feed. Since FW17 an allowed Dispense press asks the same owner for one portion, through the same readiness ([device API](device-api.md)).

What bounds a drive (provisional limits, `feeding.rs`):
- **No confirmed feedback edge:** the controller stops after `feedback_timeout_ms` (15 s) and the feed fails `NoFeedback`. In single-bowl mode a portion is one rising PA18 edge, the end of a compartment's mark, at least 5 s (`feedback_inhibit_ms`) after the drive starts or after the last portion. Stock does the same: its PA18 interrupt (`0x0e01a55c`) accepts only a HIGH read in single-bowl mode, then disarms PA18 for 5 s, as it does when the drive starts.
- **Whole feed:** 12 s a portion (`portion_ms`), capped at 48 portions' worth. **Current:** two consecutive scaled polls above 300 (stock's unit) stop it, with no reverse attempt. **Outlet** blocked or the beam unreadable: the feed expires before the motor starts.
- **Loop stalls:** the supervisor stops the motor 200 ms after the owner's last renewal; an owner beat older than 1 s while driving ends in a watchdog reset.

Each feed logs, in order: `PLOWN1 feed id= portions= submit=0`, the state changes (`state=3` preparing, `4` feeding, `5` reporting, `6` terminal, `1` idle), then
`PLOWN1 fed id= requested= completed= outcome= reason= durable= drive_ms= rising= falling= first_edge_ms= current_max= current_mean= samples=`,
which counts every PA18 edge while capture was on, qualified or not. Outcomes: 1 done, 2 cancelled, 3 expired, 4 failed, 5 uncertain (an interrupted feed, resolved at the next boot); reasons are `FeedReason` codes (0 none, 6 no feedback, 5 timeout, 10 overcurrent, 9 outlet, 12 output). Then `PLOWN1 edges id=…`, and since FW17 `PLOWN1 steps id= max_gap_ms= handshakes=`: the longest gap between two owner steps while the feed was in flight (it must stay under the 100 ms observation gap) and the handshake operations the loop ran meanwhile.

**A power cut or reset mid-feed** leaves the journal's reservation pending. The next boot resolves it: the owner writes an uncertain result for it to the journal (outcome 5, `completed=0`, the same id), sits Terminal until the loop has logged `PLOWN1 fed id=N … outcome=5 … durable=1` and recorded a `Fed` event with outcome `Uncertain` (by the feeder), then goes Idle. The feed is never retried and its id is never used again; the next feed takes the id after it. A power cut during the resolution's own journal write leaves the reservation pending, so the boot after resolves it, once. A cut while the reservation itself is being written (a few milliseconds before the motor may start) leaves a cut record beside the one at rest; the journal can only have been writing the next reservation there, so the next boot resolves it the same way, as uncertain under the next id. Usually that reservation never committed: the motor never ran and no client was answered `Feeding`, so the uncertain result is conservative. A cut late in that resolution's own erase returns the journal to the record at rest and the id is reserved again, which is harmless for an uncommitted reservation; only a committed reservation that also decayed could be forgotten that way. A journal that does not decode still faults the owner closed (`fault=2`, recovery).

### Procedure (owner present)

1. Hopper empty, outlet clear, hands away. The owner can cut USB power at any moment; the feed then reads as uncertain at the next boot (above).
2. Hotspot up; a receive-only UART capture running.
3. `wellfed feed --portions 1 --address <feeder>`; watch the mechanism and note which way it turns and whether it stops by itself.
4. Afterwards, `wellfed events` shows the `Fed` event; hotspot down.

These limits are from v168; v164–v167 had shorter or measuring ones (below).

Pass for FW16: the drive stops by itself (a portion or `NoFeedback`) with `stopped=1` and no `PLFLT`. The record gives the first drive time, edge counts and current; the direction comes from observation.

### First feed (October 25)

Physical, owner present, hopper empty, outlet clear. v164 over the hotspot, with a receive-only UART capture (`fw16-feed-02`). `wellfed feed --portions 1` answered `Feeding { feed: 1 }` and then received the `Fed` event (failed, `NoFeedback`). The UART recorded:

```
PLOWN1 feed id=1 portions=1 submit=0
PLOWN1 state=3 fault=0 supervisor=0 stopped=1 optics_off=0
PLOWN1 state=4 fault=0 supervisor=0 stopped=0 optics_off=0
PLOWN1 state=6 fault=0 supervisor=0 stopped=1 optics_off=1
PLOWN1 fed id=1 requested=1 completed=0 outcome=4 reason=6 durable=1 drive_ms=3018 rising=0 falling=0 first_edge_ms=none current_max=166 current_mean=64 samples=100
PLOWN1 state=1 fault=0 supervisor=0 stopped=1 optics_off=1
```

- **Pass:** the drive stopped by itself on the 3 s feedback timeout, `stopped=1`, no `PLFLT`; the result is durable and the journal acknowledged it.
- **Direction:** the owner saw the rotor turn clockwise (`Drive0`, PA13 high).
- **Feedback:** no PA18 edge, rising or falling, in 3 s of drive. PA18 is likely not the rotation sensor; next, the same feed with the logic analyzer on the candidate inputs.
- **Current (scaled):** max 166, mean 64 over 100 samples, empty.

### First portion (October 25)

Physical, owner present, hopper empty. Three more feeds narrowed it down:
- **Pin watch (v165):** the supervision tick samples both GPIO ports while capture is on, and each feed logs `PLOWN1 pins … changed= toggles=`. Feed 3 changed no pad in 3 s; feed 4 saw PA18 go LOW at 1,798 ms and back, and nothing else.
- **Start-up surge (v166):** feed 2 read 333 on its only current poll and stopped `Overcurrent` 17 ms in. Overcurrent now needs two consecutive polls over the limit (`OVERCURRENT_POLLS`); a feedback edge reuses its poll's sample and does not count.
- **Measuring limits (v167):** feedback timeout 15 s, feed cap 20 s, optics 22 s, and `PLOWN1 edges id= at=` with each edge's time.

```
PLOWN1 fed id=5 requested=1 completed=1 outcome=1 reason=1 durable=1 drive_ms=9072 rising=1 falling=1 first_edge_ms=7888 current_max=164 current_mean=60 samples=300
PLOWN1 edges id=5 at=7888L,9054H
```

- **Feedback:** PA18 rests HIGH and reads LOW for about 1.17 s at a mark; the portion is credited on the rise and the motor stopped 18 ms later. Feed 4 had ended just past a mark, so a portion is about 9.05 s of drive empty.
- **Mechanism:** the owner saw about an eighth of a turn in 3 s, about 24 s a turn, so likely three compartments per turn.
- **Current (scaled):** max 164, mean 60 over 9 s, empty.
- **Limits:** the 3 s timeout and 10 s cap were far too short. Proposed: a feedback timeout of about 12 s and a cap that scales with portions, once a run with food confirms the timing.

### Food and a full turn (October 25)

Physical, owner present.
- **With food (v167, `fw16-food-01`, `-02`):** three 1-portion feeds ended `Done` at 9,787–9,857 ms, mark LOW about 1.38 s, mean current 82–87. PA17 read LOW (food present). The outlet beam (PA16) changed 0, 4 and 8 times as the compartments filled; the owner saw food come out, a little less than stock's portion at first.
- **Mechanism (owner photos):** a three-compartment rotor over one drop hole in the base. Each compartment fills from the hopper and empties by gravity over the hole, so a portion is one compartment, a third of a turn.
- **Stock (static, PA18 interrupt `0x0e01a55c`):** single-bowl mode accepts PA18 only when it reads HIGH, the end of a mark, then disarms it for 5 s; the drive's start also arms that 5 s timer. Its missing-feedback protection is about 30 s.
- **Limits (v168 `0c955e25…`):** inhibit 5 s, feedback timeout 15 s, a deadline of 12 s a portion (`portion_ms`) capped at 48 portions, optics up to 578 s. The emitters' safe on-time is FW30's.
- **Three portions (`fw16-turn-01`, empty):**

```
PLOWN1 fed id=9 requested=3 completed=3 outcome=1 reason=1 durable=1 drive_ms=27241 rising=3 falling=3 first_edge_ms=7840 current_max=168 current_mean=59 samples=903
PLOWN1 edges id=9 at=7840L,9037H,16970L,18156H,26047L,27221H
```

  9.1 s a portion, the stop 20 ms after the third rise, and the owner's tape mark on the rotor back at its start: three marks a turn.

These marks, the start-up surge and the driving current are the virtual feeder's plant (`wellfed_sim::plant`), which runs the owner under these limits ([development guide](../development/README.md#work-on-a-component)).

Cancellation during motion passed on October 25 (`feedwrites-20261025-01`, v173: feed 18 cancelled 2 s into its drive, `outcome=2 reason=2`, outputs stopped). Left for FW16: the motor lease under a stalled loop during a feed, and a failed stop ([below](#a-stalled-loop-and-a-failed-stop-during-a-feed)).

## A stalled loop and a failed stop during a feed

Status: passed on hardware, October 26 (physical, unattended, hopper empty, no batteries; [result](#result-october-26)); host tests and the virtual feeder cover the same cases.

Two opt-in variants of `feeder` inject one fault into each feed, 2 s (`INJECT_AFTER_MS`) into its drive, so the motor is running and past its start-up surge. They show, with real motion, the supervision that does not depend on the loop or on the stop path: the TIM2 interrupt's motor lease and its stop retry, the owner's beat, and the runtime's watchdog. The normal `feeder` image has neither: `feed-stall` is a feature of the firmware crate only, and `stop-fail` also enables `wellfed-hal`'s `stop-fail` feature, without which the injection is not compiled. Nothing else changes: no startup motion, feeds only from the device API, the same limits, the same journal and no-retry rule. The requested portions pick the case, so the bench needs no new operation:

| Variant, request | Fault | What ends the drive | Then |
| --- | --- | --- | --- |
| `feed-stall`, 1 portion | The loop spins 600 ms, interrupts enabled | The motor lease: the timer interrupt stops the motor 200 ms after the owner's last renewal (`MotorGateFault::Expired`) | The owner, stepped again, finds its supervisor faulted and records the feed failed (`outcome=4 reason=12`) durably. A faulted owner never beats again, so 1 s after its last beat the runtime withholds the watchdog and the board resets; the next boot is Idle with nothing pending |
| `feed-stall`, 2–3 portions | The loop spins 8 s | The motor lease, as above | The beat lapses 1 s after the last one and the watchdog resets the board inside the spin; the next boot resolves the feed as uncertain (`outcome=5`) under its id and never re-runs it |
| `stop-fail`, 1 portion | STOP writes held 300 ms, then the feed cancelled | The timer interrupt's stop retry, every 10 ms, the first one after the hold lifts | The cancellation is recorded durably (`outcome=2`, owner fault hardware), then the faulted owner's lapsed beat resets the board |
| `stop-fail`, 2–3 portions | STOP writes held 8 s, then the feed cancelled | The watchdog reset: the owner stops beating once its stop fails, the runtime withholds the refresh after the 1 s gap, and the reset puts the motor pins in their reset state (the driver's standby) | The next boot resolves the feed as uncertain |

The lengths are pinned against the supervision bounds at compile time (`faults.rs`): the short ones between the 200 ms motor lease and the 1 s beat, the long ones past the beat plus the 6 s ceiling on the watchdog's expiry (its bounds are 2.4–5.5 s, measured about 4 s).

**Where the failed stop is injected, and why there.** The only place that writes the motor pins while the board is open is the board's pin write (`BoardPins::write_pair`), which writes the pair and reads it back. With a hold armed (`wellfed_hal::inject`), that write skips a STOP, leaves the pins as they are, and does the readback as always; the pins still drive, so the write fails its readback exactly as a pad that did not follow its latch would. Everything downstream is the product's own failure handling, unchanged: the gate latches an I/O fault with the applied state unknown, the supervisor faults its outputs, the timer interrupt retries the stop every tick, the owner faults hardware, keeps its result back until a stop verifies, and stops beating; the runtime's watchdog check reads the pins (`stopped::observe`) and the beat. This point was chosen over the others:
- **Skipping the owner's stop request** (the controller or `quiet`) would only exercise the owner: the gate would still stop the motor at its lease, so nothing independent would be shown.
- **Re-driving the pins behind the board's back** (a write from the loop after the stop) bypasses the readback the board relies on, and models a second writer the board does not have; the gate would believe STOP and only the runtime's pin check would catch it.
- **Changing pin modes or masking the timer** puts the driver's inputs or the supervision itself in a state the product never sees, and could leave the motor in an unknown mode.

The hold only skips STOP; a drive write is untouched, and the faulted gate issues none. It is bounded twice: the variant asks for 300 ms or 8 s, and the SDK caps any hold at 10 s (`MAX_HOLD_MS`). After the hold lifts, the timer's next retry stops the motor even if the watchdog never reset, so the long case's drive is at most about 10 s (2 s before the injection plus the 8 s hold), whatever else fails. The hold is a static in RAM and starts disarmed at every boot.

Both variants inject into every feed they run, whatever starts it (the device API, and the Dispense button or a schedule once those feed), so never leave one installed: put the normal `feeder` back right after the run.

**The stall** spins in the loop task with interrupts enabled, like `owner-stall`, so the TIM2 interrupt and the runtime's watchdog feeder keep running while the owner is not stepped.

### Records

Besides the usual `PLOWN1` lines (the state changes, `fed`, `edges`, `pins`) and the runtime's `PLSUP1 stop … owner_age= owner_gap= supervised=` when it withholds the watchdog:
- `PLINJ1 inject id= fault= severity= ms=`: fault 1 stall, 2 failed stop; severity 1 short, 2 long; `ms` the spin or the hold.
- `PLINJ1 stall id= end ms=`: the loop came back from its spin. After a long stall this line means the watchdog did not reset the board: a failure.
- `PLINJ1 stop id= cancel=`: the hold is armed and the feed cancelled.
- `PLINJ1 hold lifted= skipped=`: the hold ended by itself, and how many STOP writes it skipped (the timer's retries, about one per 10 ms, and the owner's). After a long hold this line means the watchdog did not reset the board: a failure.

### Host and simulation coverage

- `wellfed-hal`'s `tests/inject.rs` (built with `--features stop-fail`; `make check-rust` runs it): a held stop keeps driving and the owner holds its result and stops beating; the first timer retry after the hold stops the motor and the cancellation is recorded durably; a hold is capped and never touches a drive write.
- The virtual feeder (`wellfed-sim`, the `stop_fails` command models the same failed readback): a 600 ms stall stops by the lease within 220 ms and records the feed failed, then resets with nothing pending; a 300 ms failed stop is stopped by the first retry after it and recorded cancelled; a stop that never takes drives until the watchdog reset, which comes 1 s plus an expiry inside the bounds after the cancel, and the next boot resolves the feed as uncertain. The 8 s stall was already covered (`stalled_owner_independent_timer_and_watchdog_reset`).
- `faults.rs`: each feed is injected once, 2 s into its own drive, with the severity its portions choose.

### Bench plan (unattended motion: hopper empty, D cells out, at most 3 portions)

About 45 minutes: three over-the-air installs to bank 1 (no bank-0 write) and four feeds, each driving at most about 10 s.

1. **Before.** `wellfed-bench status`: relay 1 on, UART relays closed, no capture or server running; the hotspot up; the feeder answers at its address with the `feeder` build confirmed on bank 1. Note the journal's high water from `wellfed status`.
2. **Install `feed-stall`:** `wellfed-bench ota fw16-stall-20261026-01 --build --firmware feeder --variant feed-stall` from the main checkout's root (or `--root` to it), and wait for the build to confirm. A reset during the trial would count against it, so no feed runs before the confirmation.
3. **Each feed** (names `fw16-stall-20261026-NN`, `fw16-stopfail-20261026-NN`):
   1. Start a 180 s UART capture: `wellfed-bench capture NAME-uart --seconds 180`.
   2. Start the logic analyzer on all eight channels: `sigrok-cli --driver fx2lafw --config samplerate=1M --time 60000 --channels D0,D1,D2,D3,D4,D5,D6,D7 --output-format binary --output-file private/captures/NAME-la.bin`.
   3. `wellfed feed --portions N --address 10.42.103.172`. In a long case the board resets mid-feed and the client loses its session without a `Fed` event: the feed is uncertain, and is never sent again.
   4. Watch the UART for the expected records. **Stop condition:** a drive still running 1 s after it should have ended (3 s after `PLOWN1 state=4` for the stalls and the short failed stop, 11 s for the long failed stop, past the hold's own bound), or any motion after a reset, means power off at once: `wellfed-bench off` (after the capture ends, or `boot` cuts power itself), then stop and report.
   5. After the reset, the next boot must show `PLOWN1 open` (the hooks armed) before anything else runs. If arming was skipped (a store write was in progress at the reset, the once-per-power-cycle rule), a `wellfed-bench boot` power cycle lets the owner open; the pending feed then resolves.
   6. `wellfed events --address …` shows the `Fed` event: failed, cancelled or uncertain as the table says.
4. **Order:** `feed-stall` with 1 portion, then 2; install `stop-fail` (`fw16-stopfail-20261026-01`), then 1 portion, then 2.
5. **Restore:** install the normal `feeder` from the branch's base (`--build --firmware feeder`), wait for its confirmation, check `wellfed-bench status`, and leave the hotspot up with no capture or server running.

**Pass:** for every feed, the analyzer shows one drive (D3 HIGH, clockwise) that ends where the table says: about 2.2 s for both stalls (the lease), about 2.3 s for the short failed stop, and at the reset for the long one (2 s plus at most about 7 s); the UART shows the injection record, the stop or reset, and the expected `fed` record on the right boot, with no second drive after a reset and no `PLINJ1 stall … end` or `hold lifted` after a long case. A flat analyzer capture through a drive the firmware reports is a failed capture, not evidence.

### Result (October 26)

Physical, unattended, hopper empty, adapter only (health `battery_mv` null). `feed-stall` v228 (`7e367b6a…`) and `stop-fail` v229 (`5166022b…`) each installed over the air to bank 1 and confirmed before any feed; the normal `feeder` v230 (`aa3c7f3f…`, the same branch) was restored and confirmed afterwards. Analyzer at the motor, 1 MHz, all eight channels; UART times are host timestamps from `PLOWN1 state=4` (drive start).

| Feed | Case | Injection (UART) | Drive at the motor (D3 HIGH, D2 LOW) | Result | Reset |
| --- | --- | --- | --- | --- | --- |
| 26 (`fw16-stall-20261026-02`) | stall 600 ms | 2.03 s | 2,210 ms: ended about 0.18 s after the injection, by the lease | `outcome=4 reason=12 durable=1` (failed, output), `Fed` received by the client | `PLSUP1 stop owner_age=1116`, reset 3.8 s later; next boot Idle, high water 26 |
| 27 (`fw16-stall-20261026-03`) | stall 8 s | 2.05 s | 2,227 ms: by the lease | No `stall end`: the watchdog reset the board inside the spin (`PLSUP1 stop owner_age=1043`, reset 3.8 s later). The client lost its session; next boot `fed id=27 outcome=5` (uncertain, by the feeder) | Yes |
| 28 (`fw16-stopfail-20261026-02`) | failed stop, 300 ms | 2.03 s | 2,317 ms: about 0.29 s after the injection, at the first retry after the hold (`hold lifted=1 skipped=60`: the timer interrupt retries once per 10 ms tick, about 29 of them; the rest are the owner's own stop attempts from the loop, static) | `outcome=2 durable=1` (cancelled), owner fault hardware, `Fed` received | `PLSUP1 stop owner_age=1076`, reset 3.8 s later; next boot Idle |
| 29 (`fw16-stopfail-20261026-03`) | failed stop, 8 s | 2.01 s | 6,995 ms: until the reset, about 5 s after the injection | `PLSUP1 stop outputs=0 owner_age=1172`, reset 3.8 s later, no `hold lifted`; next boot `fed id=29 outcome=5` (uncertain) | Yes |

- Each capture shows one drive and none after a reset; the next boots went from standby to brake with no differing read. At feed 29's reset (`fw16-stopfail-20261026-03-la.bin`), the long drive ended and then the two leads read differently three more times within 0.2 ms: 38 µs with D2 HIGH and D3 LOW (the opposite of the clockwise drive), then 38 µs and 110 µs with D3 HIGH and D2 LOW (the drive's polarity). No drive followed (physical). One possible reading, static: back-EMF from the coasting motor across leads the driver no longer holds once the GPIOs reset. Back-EMF would keep the drive's polarity, though, so the opposite-polarity read does not fit it, and outputs-only captures cannot tell drive from noise near the threshold; only the driver's inputs can ([motor driver](../devices/plaf103/motor-driver.md#startup-transients-at-the-motor-october-24)). The transition when a reset lands mid-drive is open in FW30's remaining checks.
- No boot skipped arming; every interrupted feed resolved at the next boot and none was re-run. The client was not retried. Journal high water 29.
- The four resets each came 3.80–3.83 s after the runtime withheld the refresh, which it had last given about 250 ms earlier (`fed_ms`): about 4.05 s from the last refresh, the 32.8 kHz of October 25.
- The `Fed` events for feeds 26–29 are in the event history: failed, uncertain, cancelled, uncertain.

### Carried items owned by FW16

| Item | Where it stands |
| --- | --- |
| Production watchdog clock bounds | The October 25 resets put the clock near 32.8 kHz (physical); the bounds stay 24–54 kHz. FW16 depends only on the wide bound: the long injections are sized past its 6 s ceiling. The October 26 runs added four reset timings, each about 4.05 s from the last refresh (32.4–32.8 kHz). Narrowing the bounds is not needed for FW16; carry it to FW19, whose stop timing under motion uses them |
| Owner-lease expiry while the loop stalls, on hardware | Closed (physical, October 26): both stalls ended the drive about 0.2 s after the injection, by the lease ([result](#result-october-26)) |
| PA18 feedback | Closed (physical, October 25): PA18 is the rotor's mark sensor, one mark per portion, credited on its rise. Counting food through the outlet beam (PA16 changed as food fell, not yet qualified) goes to FW18, which measures delivered food |
| Motor current on ADC channel 4 | Loaded readings exist (physical): scaled 59–64 mean, about 166 max empty, 82–87 mean with food, a 333 start-up surge on one poll, now ignored unless two polls agree. Scaling to milliamps needs a reference measurement of the motor's current (a meter or USB power meter in series), which is the owner's; with it, the thresholds go to FW19 |
| Journal reservation before motion and result after a verified stop | Closed (physical, October 25): every owner feed reserved before driving and finished durably after a verified stop, cancellation during motion included, and an interrupted feed resolves as uncertain at the next boot (AR24). The failed-stop runs add the case where the stop does not verify at once |

**Static finding:** a supervision reset that lands while a store write holds the flash (the event ring's write of the `Fed` event, in the seconds between a short case's result and its reset) leaves the attempt marker set, so the next boot skips arming and the owner does not open until a power cycle. The product's resets have the same exposure; the bench plan checks for it. **Decided (October 26, static analysis and host tests):** a reset the runtime withholds no longer lands in a store write it let start. From the first feeder pass that does not feed the watchdog, no write turn is granted, no arming starts and an install goes no further (`supervision::writes_held`); a write already running finishes in tens of milliseconds, well inside the watchdog's expiry, and clears the marker itself. So the marker is left set only by a reset the runtime did not decide (a hang, a fault, a halted lease), which is the case it exists for, and nothing needs to clear it ([OTA design](ota-design.md#the-attempt-marker-and-decided-resets)).


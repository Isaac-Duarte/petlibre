# FW16 first supervised motor pulse

Status: run once on hardware, October 24 (owner present; [result](#first-run-october-24)). The owner chose to go ahead before FW30's power-on flicker is classified, since it also appears in the ROM loader.

## What the image does

`wellfed-build build --profile firstboot --firmware motor --variant pulse` builds the `motor` firmware's `pulse` variant. It records `startup_motion_policy: fw16-pulse-v1` (declared in `src/firmware/wellfed/motor/Cargo.toml`) and `variant: pulse` in the manifest and gets its own build ID. The motor firmware's normal image adds no task, and the builder refuses a motion policy outside a variant, so nothing else can drive the motor. The bench never builds it.

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

Lease expiry with a stalled owner was host-tested and FW05-measured, but not under real motion; this session gives the first real-motion data. Cancellation, failed stop, the journal-reserved feed, portions and reverse recovery belong to the full FW16 owner path and FW19. That owner runs inside `feeder`'s loop, and since v164 the device API feeds through it, so later motor tests run over the air: see [feeds through the owner](#feeds-through-the-owner).

## Feeds through the owner

Since v164 (`469935b4…`, October 25) `wellfed feed --portions N` runs a feed through `feeder`'s feeding owner: the journal reservation, the supervisor's 200 ms motor lease, the feed limits in `feeding.rs`, the result in the journal, and a `Fed` event. Nothing else moves the motor: the image has no startup motion, the bench never sends a feed, and the Dispense button is not wired to it yet (FW20).

What bounds a drive (provisional limits, `feeding.rs`):
- **No confirmed feedback edge:** the controller stops after `feedback_timeout_ms` (15 s) and the feed fails `NoFeedback`. In single-bowl mode a portion is one rising PA18 edge, the end of a compartment's mark, at least 5 s (`feedback_inhibit_ms`) after the drive starts or after the last portion. Stock does the same: its PA18 interrupt (`0x0e01a55c`) accepts only a HIGH read in single-bowl mode, then disarms PA18 for 5 s, as it does when the drive starts.
- **Whole feed:** 12 s a portion (`portion_ms`), capped at 48 portions' worth. **Current:** two consecutive scaled polls above 300 (stock's unit) stop it, with no reverse attempt. **Outlet** blocked or the beam unreadable: the feed expires before the motor starts.
- **Loop stalls:** the supervisor stops the motor 200 ms after the owner's last renewal; an owner beat older than 1 s while driving ends in a watchdog reset.

Each feed logs, in order: `PLOWN1 feed id= portions= submit=0`, the state changes (`state=3` preparing, `4` feeding, `5` reporting, `6` terminal, `1` idle), then
`PLOWN1 fed id= requested= completed= outcome= reason= durable= drive_ms= rising= falling= first_edge_ms= current_max= current_mean= samples=`,
which counts every PA18 edge while capture was on, qualified or not. Outcomes: 1 done, 2 cancelled, 3 expired, 4 failed; reasons are `FeedReason` codes (6 no feedback, 5 timeout, 10 overcurrent, 9 outlet, 12 output).

**A power cut or reset mid-feed** leaves the journal's reservation pending; the owner then faults at every boot (`fault=2`, recovery) and refuses feeds `Fault` until FW17 resolves an interrupted feed as uncertain. Avoid cutting power during a feed.

### Procedure (owner present)

1. Hopper empty, outlet clear, hands away. The owner can cut USB power at any moment, but see the mid-feed note above.
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

Left for FW16: cancellation during motion, the motor lease under a stalled loop during a feed, and a failed stop.


# Motor and feeds

One motor turns the feeder's rotor, and only one piece of firmware may drive it: the [feeding owner](../glossary.md). Every feed, from the app, the CLI or the Dispense button, is a request to that owner. This page explains what bounds a drive, what a feed records, what happens when power is cut mid-feed, and the fault-injection variants that prove the motor stops even when the loop or the stop path fails. It ends with the first-pulse probe that brought the motor up.

Status: **Works**. Feeds through the owner, stopping on time and surviving a power cut without feeding twice, are observed on our unit.

## How a feed drives the motor

`wellfed feed --portions N` runs a feed through the `feeder` firmware's feeding owner:

1. The feed journal reserves the feed before the motor may start.
2. The motor drives clockwise (`Drive0`, PA13 HIGH and PA14 LOW) under the supervisor's 200 ms motor lease.
3. The owner credits portions from the rotor's mark sensor (PA18) and stops when the feed is done or a limit ends it.
4. The result goes to the journal once STOP is verified, and becomes a `Fed` event.

Nothing else moves the motor. The image has no startup motion, and the bench never sends a feed. An allowed Dispense press asks the same owner for one portion, through the same readiness check ([device API](device-api.md)).

What bounds a drive (limits in `feeding.rs`):

- **Portions.** In single-bowl mode a portion is one rising PA18 edge, the end of a compartment's mark, at least 5 s (`feedback_inhibit_ms`) after the drive starts or after the last portion. Stock does the same. After a feed that stopped between marks, the next feed's first rise inside those 5 s is a homing mark, never credited, and that feed's deadline is 5 s longer ([calibration](calibration.md#phase)).
- **Feedback.** No credited portion within `feedback_timeout_ms` (15 s) fails the feed `NoFeedback`. No PA18 edge for 11 s, or PA18 LOW for 4 s, also ends the drive ([jams and sensor faults](jams-and-sensor-faults.md#missing-or-stuck-feedback)).
- **Whole feed.** 12 s a portion (`portion_ms`), capped at 48 portions' worth.
- **Current.** A jam (over 300 scaled, held past the 150 ms after any change of drive) backs off 1 s and drives on, three times at most, then ends `Overcurrent`. One reading over 1000 stops at once ([jams](jams-and-sensor-faults.md#jams-bounded-reversals)).
- **Inputs.** A blocked outlet, an unreadable beam, low power or current over the limit at rest keeps a feed from starting, and it fails with that input's reason.
- **Loop stalls.** The supervisor stops the motor 200 ms after the owner's last renewal. An owner beat older than 1 s while driving ends in a watchdog reset.

### What a feed records

Each feed logs, in order:

1. `PLOWN1 feed id= portions= submit=0`.
2. The state changes: `state=3` preparing, `4` feeding, `5` reporting, `6` terminal, `1` idle.
3. `PLOWN1 fed id= requested= completed= outcome= reason= durable= drive_ms= rising= falling= first_edge_ms= current_max= current_mean= samples=`, which counts every PA18 edge while capture was on, qualified or not. Jam handling appends `reversals=`.
4. `PLOWN1 edges id=…`, with each edge's time.
5. `PLOWN1 steps id= max_gap_ms= handshakes=`: the longest gap between two owner steps while the feed was in flight, which must stay under the 100 ms observation gap, and the handshake operations the loop ran meanwhile.
6. `PLOWN1 marks id=N homed_ms=… outlet_breaks=… phase=at_mark|unknown` ([calibration](calibration.md)).

Outcomes are 1 done, 2 cancelled, 3 expired, 4 failed and 5 uncertain (an interrupted feed, resolved at the next boot). Reasons are `FeedReason` codes: 0 none, 5 timeout, 6 no feedback, 9 outlet, 10 overcurrent, 12 output.

A pin watch samples both GPIO ports on the supervision tick while capture is on, and each feed logs `PLOWN1 pins … changed= toggles=`.

### A power cut mid-feed

A power cut or reset mid-feed leaves the journal's reservation pending. The next boot resolves it:

1. The owner writes an uncertain result to the journal: outcome 5, `completed=0`, the same id.
2. It sits Terminal until the loop has logged `PLOWN1 fed id=N … outcome=5 … durable=1` and recorded a `Fed` event with outcome `Uncertain`, by the feeder.
3. It goes Idle.

The feed is never retried, and its id is never used again; the next feed takes the id after it.

The edge cases resolve the same way:

- **A cut during the resolution's own journal write** leaves the reservation pending, so the boot after resolves it, once.
- **A cut while the reservation itself is being written**, a few milliseconds before the motor may start, leaves a cut record beside the one at rest. The journal can only have been writing the next reservation there, so the next boot resolves it as uncertain under the next id. Usually that reservation never committed: the motor never ran and no client was answered `Feeding`, so the uncertain result is conservative.
- **A cut late in that resolution's own erase** returns the journal to the record at rest, and the id is reserved again. That is harmless for an uncommitted reservation. Only a committed reservation that also decayed could be forgotten that way.
- **A journal that does not decode** still faults the owner closed (`fault=2`, recovery).

A reset the runtime decides no longer lands in a store write it let start. From the first feeder pass that does not feed the watchdog, no write turn is granted, no arming starts and an install goes no further (`supervision::writes_held`). A write already running finishes in tens of milliseconds, well inside the watchdog's expiry, and clears its attempt marker itself. So the marker is left set only by a reset the runtime did not decide, such as a hang, a fault or a halted lease, which is the case it exists for ([OTA design](ota-design.md)).

## A stalled loop and a failed stop during a feed

Two opt-in variants of `feeder` inject one fault into each feed, 2 s (`INJECT_AFTER_MS`) into its drive, when the motor is running and past its start-up surge. They show, with real motion, the supervision that does not depend on the loop or on the stop path: the TIM2 interrupt's motor lease and its stop retry, the owner's beat, and the runtime's watchdog.

The normal `feeder` image has neither. `feed-stall` is a feature of the firmware crate only, and `stop-fail` also enables `wellfed-hal`'s `stop-fail` feature, without which the injection is not compiled. Nothing else changes: no startup motion, feeds only from the device API, the same limits, the same journal and no-retry rule. The requested portions pick the case:

| Variant, portions | Fault | What ends the drive | Then |
| --- | --- | --- | --- |
| `feed-stall`, 1 | The loop spins 600 ms, interrupts enabled | The motor lease: the timer interrupt stops the motor 200 ms after the owner's last renewal (`MotorGateFault::Expired`) | The owner finds its supervisor faulted and records the feed failed (`outcome=4 reason=12`) durably. It never beats again, so 1 s later the runtime withholds the watchdog and the board resets. The next boot is Idle with nothing pending |
| `feed-stall`, 2–3 | The loop spins 8 s | The motor lease, as above | The beat lapses and the watchdog resets the board inside the spin. The next boot resolves the feed as uncertain (`outcome=5`) and never re-runs it |
| `stop-fail`, 1 | STOP writes held 300 ms, then the feed cancelled | The timer interrupt's stop retry, every 10 ms, the first one after the hold lifts | The cancellation is recorded durably (`outcome=2`, owner fault hardware), then the lapsed beat resets the board |
| `stop-fail`, 2–3 | STOP writes held 8 s, then the feed cancelled | The watchdog reset: the owner stops beating once its stop fails, the runtime withholds the refresh after the 1 s gap, and the reset puts the motor pins in their reset state (the driver's standby) | The next boot resolves the feed as uncertain |

The lengths are pinned against the supervision bounds at compile time (`faults.rs`). The short ones fall between the 200 ms motor lease and the 1 s beat. The long ones go past the beat plus the 6 s ceiling on the watchdog's expiry, whose bounds are 2.4–5.5 s (measured about 4 s).

**Where the failed stop is injected.** The only place that writes the motor pins while the board is open is the board's pin write (`BoardPins::write_pair`), which writes the pair and reads it back. With a hold armed (`wellfed_hal::inject`), that write skips a STOP, leaves the pins as they are, and reads back as always. The pins still drive, so the write fails its readback exactly as a pad that did not follow its latch would. Everything downstream is the product's own failure handling: the gate latches an I/O fault with the applied state unknown, the supervisor faults its outputs, the timer interrupt retries the stop every tick, and the owner faults hardware, keeps its result back until a stop verifies, and stops beating. The runtime's watchdog check reads the pins (`stopped::observe`) and the beat.

Other injection points were rejected:

- **Skipping the owner's stop request** (the controller or `quiet`) would only exercise the owner. The gate would still stop the motor at its lease, so nothing independent would be shown.
- **Re-driving the pins behind the board's back** bypasses the readback the board relies on, and models a second writer the board does not have.
- **Changing pin modes or masking the timer** puts the driver's inputs or the supervision itself in a state the product never sees, and could leave the motor in an unknown mode.

The hold only skips STOP. A drive write is untouched, and the faulted gate issues none. It is bounded twice: the variant asks for 300 ms or 8 s, and the SDK caps any hold at 10 s (`MAX_HOLD_MS`). After the hold lifts, the timer's next retry stops the motor even if the watchdog never reset. So the long case's drive is at most about 10 s, whatever else fails. The hold is a static in RAM and starts disarmed at every boot.

The stall spins in the loop task with interrupts enabled, like `owner-stall`, so the TIM2 interrupt and the runtime's watchdog feeder keep running while the owner is not stepped.

> [!WARNING]
> Both variants inject into every feed they run, whatever starts it: the device API, the Dispense button, or a schedule once those exist. Never leave one installed. Put the normal `feeder` back right after the run.

### Records

Besides the usual `PLOWN1` lines and the runtime's `PLSUP1 stop … owner_age= owner_gap= supervised=` when it withholds the watchdog:

- `PLINJ1 inject id= fault= severity= ms=`: fault 1 stall, 2 failed stop; severity 1 short, 2 long; `ms` the spin or the hold.
- `PLINJ1 stall id= end ms=`: the loop came back from its spin. After a long stall this line means the watchdog did not reset the board: a failure.
- `PLINJ1 stop id= cancel=`: the hold is armed and the feed cancelled.
- `PLINJ1 hold lifted= skipped=`: the hold ended by itself, and how many STOP writes it skipped. After a long hold this line is a failure too.

### Rerunning the check

Unattended motion: hopper empty, D cells out, at most 3 portions. About 45 minutes: three over-the-air installs to bank 1 (no bank-0 write) and four feeds, each driving at most about 10 s.

1. Run `wellfed-bench status`. You should see relay 1 on, UART relays closed and no capture or server running, with the hotspot up and the `feeder` build confirmed on bank 1. Note the journal's high water from `wellfed status`.
2. Run `wellfed-bench ota NAME --build --firmware feeder --variant feed-stall` from the main checkout's root (or with `--root`), and wait for the build to confirm. A reset during the trial would count against it, so run no feed before the confirmation.
3. For each feed:
   1. `wellfed-bench capture NAME-uart --seconds 180` starts a UART capture.
   2. `sigrok-cli --driver fx2lafw --config samplerate=1M --time 60000 --channels D0,D1,D2,D3,D4,D5,D6,D7 --output-format binary --output-file private/captures/NAME-la.bin` starts the logic analyzer on all eight channels.
   3. `wellfed feed --portions N --address <feeder>`. In a long case the board resets mid-feed and the client loses its session without a `Fed` event. The feed is uncertain and is never sent again.
   4. Watch the UART for the expected records.
   5. After a reset, the next boot must show `PLOWN1 open` before anything else runs. If arming was skipped because a store write was in progress at the reset, a `wellfed-bench boot` power cycle lets the owner open, and the pending feed then resolves.
   6. `wellfed events --address …` shows the `Fed` event, as the table says.
4. Order: `feed-stall` with 1 portion, then 2; install `stop-fail`, then 1 portion, then 2.
5. Install the normal `feeder` (`--build --firmware feeder`), wait for its confirmation, check `wellfed-bench status`, and leave the hotspot up with no capture or server running.

> [!WARNING]
> Stop condition: a drive still running 1 s after it should have ended means power off at once with `wellfed-bench off` (or `boot`, which cuts power itself), then stop and report. That is 3 s after `PLOWN1 state=4` for the stalls and the short failed stop, and 11 s for the long failed stop. Any motion after a reset is the same.

You should see, for every feed, one drive on the analyzer (D3 HIGH, clockwise) that ends where the table says: about 2.2 s for both stalls, about 2.3 s for the short failed stop, and at the reset for the long one (2 s plus at most about 7 s). The UART shows the injection record, the stop or reset, and the expected `fed` record on the right boot, with no second drive after a reset and no `PLINJ1 stall … end` or `hold lifted` after a long case. A flat analyzer capture through a drive the firmware reports is a failed capture, not evidence.

## The first-pulse probe

Before feeds existed, the motor was brought up by one supervised 100 ms pulse. `wellfed-build build --profile firstboot --firmware bringup --variant pulse` builds it. The manifest records `startup_motion_policy: fw16-pulse-v1` (declared in `src/firmware/wellfed/bringup/Cargo.toml`) and `variant: pulse`, and the build gets its own build ID. The builder refuses a motion policy outside a variant, and the bring-up firmware's other images start no motor task, so nothing else can drive the motor. The bench never builds it.

After startup, the pulse probe (`src/sdk/wellfed-probes/src/pulse.rs`):

1. Skips everything unless this is a clean cold boot: reset reason 0 and its backup-register marker clear.
2. Opens the board (motor STOP, emitters OFF, sensors), binds a fresh flash authority to the KM0 peer, and opens the supervisor: TIM2 at 10 ms, owner and motor lease 200 ms. It enables feedback capture.
3. Logs `armed`, then waits 60 s, renewing its owner lease every 2 ms, so the operator can get ready.
4. Sets the marker, then drives once: `DRIVE_0` (PA13 HIGH, PA14 LOW) for 100 ms, then STOP. It counts PA18 edges and samples motor current on ADC channel 4 every 10 ms.
5. Watches for 1 s more, then verifies STOP: the gate state, the motor pins, the supervisor closing and stopped-output readback.

The motor gate's 200 ms command lease and the owner lease, both enforced by the timer interrupt, hold even if the task stalls. A host test stalls the task mid-pulse and requires STOP within the lease. A drive is applied only through `Events::motor`, which needs a running timer, a live owner and enabled capture.

> [!WARNING]
> Every clean cold boot pulses once. The marker survives a watchdog reset but not power loss, so the image must not stay installed after the session.

Records (`PLMOT1`):

- `start created=`
- `open board= ready= attached= events= capture= ms=`
- `armed delay_ms= pulse_ms= lease_ms=`
- `pulse dir= applied_ms= on_ms= edges= first_edge_ms= current=min,max,mean samples=`
- `after ms= edges= current=… stopped=`
- `done`
- `skip`, `timeout` or `error stage= fault= stopped=`, which end the probe stopped.

Guards:

- `wellfed-bench flash-prepare` and `flash-execute` refuse a target whose manifest names a motion policy unless `--attended-motion` is given.
- The startup audit binds `fw16-pulse-v1` to the `PLMOT1 start created=` format both ways, and refuses motion and optics policies together.
- The electrical output checks and the current channel were prerequisites.

Procedure, with the owner present:

1. Check the prerequisites: the transient check passes (`wellfed-bench transients`), and the current channel reads apart from the battery channel.
2. Empty the hopper, clear the outlet, keep hands away. The owner can cut USB power at any moment.
3. Install the frozen review with `--attended-motion` and a short `--boot-seconds` (10). Power off right after the install.
4. Load a dashboard script anchored on `PLMOT1 armed`, with a countdown to the pulse at 60 s.
5. Cold boot with a 120 s capture. Observe the motion and record its direction and size.
6. Reinstall a non-motion image before anything else.

You should see one `pulse` record with `on_ms` about 100 and `stopped=1` after it, the motor moving only once, and no `error`.

## Evidence

All observed on the research unit with the hopper empty, unless marked. The full dated record is kept in the project's work history.

- **First pulse** (October 24, owner present, `fw16-pulse-01`, image `073940ec…`, capture `fw16-pulse-01-run`). One pulse of 106 ms (the 10 ms timer tick), `stopped=1`, no error. The owner heard the motor. On the logic analyzer at the motor (1 MHz), the only drive in 130 s was 106.07 ms with the white-lead side HIGH, 60.5 s after power-on; `dir=1` is `MotorCommand::Drive0`, PA13 HIGH and PA14 LOW. Opening the board before arming showed a 212 µs coast. Motor current read raw 64–447, mean 147 over 10 samples while driving, and 15–16 stopped. No PA18 edge in the pulse or the second after it. `feeder` v154 was restored over UART (`fw16-restore-01`). The owner chose to run this before the power-on flicker at the motor was classified, since that flicker also appears in the ROM loader.
- **Direction** (October 25, `fw16-feed-02`, v164). The first feed through the owner: the owner saw the rotor turn clockwise (`Drive0`, PA13 HIGH). It stopped by itself on the then 3 s feedback timeout (`outcome=4 reason=6 durable=1 drive_ms=3018`, current max 166 and mean 64 scaled), with no `PLFLT`, and the client got the `Fed` event.
- **Mark sensor** (October 25, v165–v167). The pin watch saw only PA18 change among the GPIO pads. PA18 rests HIGH and reads LOW for about 1.17 s at a mark; the portion is credited on the rise and the motor stopped 18 ms later (`drive_ms=9072`, edges at 7,888 ms LOW and 9,054 ms HIGH). A portion is about 9.05 s of drive, empty. A start-up surge read 333 on one poll and tripped a feed 17 ms in, so overcurrent needs two consecutive polls (`OVERCURRENT_POLLS`); a feedback edge reuses its poll's sample and does not count.
- **Food and a full turn** (October 25). With food (v167, `fw16-food-01`, `-02`), three 1-portion feeds ended `Done` at 9,787–9,857 ms, mark LOW about 1.38 s, mean current 82–87. PA17 read food present, the outlet beam changed 0, 4 and 8 times, and food came out, a little less than stock's portion at first. A 3-portion feed (`fw16-turn-01`, v168 `0c955e25…`) took 27,241 ms with marks at 9.0, 18.2 and 27.2 s, stopping 20 ms after the third rise, and the owner's tape mark returned to its start: three compartments a turn. Owner photos show a three-compartment rotor over one drop hole. These marks, the surge and the driving current are the virtual feeder's plant (`wellfed_sim::plant`) ([development guide](../development/README.md)).
- **Stock** (recovered, PA18 interrupt `0x0e01a55c`): single-bowl mode accepts PA18 only when it reads HIGH, the end of a mark, then disarms it for 5 s; the drive's start also arms that timer. Its missing-feedback protection is about 30 s.
- **Cancellation** (October 25, `feedwrites-20261025-01`, v173): feed 18 cancelled 2 s into its drive ended `outcome=2 reason=2` with outputs stopped.
- **Power cut mid-drive** (October 25 and 26): an interrupted feed resolved as uncertain at the next boot and was not re-run.
- **Stall and failed stop** (October 26, unattended, adapter only, analyzer at the motor). `feed-stall` v228 (`7e367b6a…`) and `stop-fail` v229 (`5166022b…`) each installed to bank 1 and confirmed before any feed; `feeder` v230 (`aa3c7f3f…`) was restored afterwards.

  | Feed | Case | Drive at the motor | Result |
  | --- | --- | --- | --- |
  | 26 (`fw16-stall-20261026-02`) | Stall 600 ms | 2,210 ms, about 0.18 s after the injection, by the lease | Failed, output, durable; reset 3.8 s after `PLSUP1 stop owner_age=1116`; next boot Idle |
  | 27 (`fw16-stall-20261026-03`) | Stall 8 s | 2,227 ms, by the lease | Watchdog reset inside the spin; next boot `outcome=5` |
  | 28 (`fw16-stopfail-20261026-02`) | Failed stop 300 ms | 2,317 ms, at the first retry after the hold (`hold lifted=1 skipped=60`) | Cancelled, durable, owner fault hardware; then reset |
  | 29 (`fw16-stopfail-20261026-03`) | Failed stop 8 s | 6,995 ms, until the reset | `PLSUP1 stop outputs=0 owner_age=1172`; next boot `outcome=5` |

  Each capture shows one drive and none after a reset, and no boot skipped arming. The four resets came 3.80–3.83 s after the runtime withheld the refresh, about 4.05 s from the last refresh: about 32.8 kHz. Of the 60 skipped STOP writes, about 29 were the timer's retries (one per 10 ms tick) and the rest the owner's own attempts (recovered).
- **Reset mid-drive** (October 26, `fw16-stopfail-20261026-03-la.bin`). At feed 29's reset the long drive ended, then the two leads read differently three more times within 0.2 ms: 38 µs with D2 HIGH and D3 LOW (opposite to the drive), then 38 µs and 110 µs with the drive's polarity. No drive followed. Back-EMF from the coasting motor would keep the drive's polarity, so the opposite read does not fit it, and outputs-only captures cannot tell drive from noise near the threshold ([motor driver](../devices/plaf103/motor-driver.md)). This transition is still open.
- **Hosts and simulation.** Modeled: `wellfed-hal`'s `tests/inject.rs` (built with `--features stop-fail`; `make check-rust` runs it) shows a held stop keeps driving while the owner holds its result and stops beating, the first retry after the hold stops the motor and the cancellation is recorded, and a hold is capped and never touches a drive write. The virtual feeder (`wellfed-sim`, its `stop_fails` command) stops a 600 ms stall by the lease within 220 ms, stops a 300 ms failed stop at the first retry, and drives a never-taking stop until the watchdog reset, 1 s plus an expiry inside the bounds after the cancel. The 8 s stall is covered by `stalled_owner_independent_timer_and_watchdog_reset`. `faults.rs` injects each feed once, 2 s into its own drive.
- **Current in milliamps.** Not measured. Scaling the channel to milliamps needs a reference measurement of the motor's current (a meter or USB power meter in series).

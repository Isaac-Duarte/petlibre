# Jams, missing feedback and sensor faults

A feed can go wrong: kibble jams the rotor, the mark sensor stops reporting, the outlet is blocked, or the ADC stops converting. In every case the feeding owner stops the motor, writes an explicit outcome and reason to the journal and the `Fed` event, and never restarts the feed by itself. A client never resends an uncertain feed, and the feeder never re-runs one.

Status: **Experimental**. Every case runs on hardware through fault-injection variants. Real jams with food, taped sensors and a current reference wait for the owner ([left for the owner](#left-for-the-owner)).

The limits are in `feeding::CONFIG` (`src/firmware/wellfed/feeder/src/feeding.rs`). The behavior is the controller's (`wellfed_core::feed::controller`) and the owner's (`wellfed_hal::owner`). The drive itself, its marks and its current are in [motor and feeds](motor-pulse.md#how-a-feed-drives-the-motor).

## Jams: bounded reversals

A jam is motor current held over the limit: two consecutive polls, 20 ms apart, over 300 scaled, outside the 150 ms after any change of drive. The controller then backs off: 1 s the other way, a 0.5 s stop, and forward again. It does this at most three times, as stock does. After the third, the next jam ends the feed `Failed`, `Overcurrent`.

| Limit | Value | Why |
| --- | --- | --- |
| `current_limit_ma` | 300 scaled | Stock's limit, in stock's unit. 1.8 times the highest drive peak seen |
| `surge_ms` | 150 | Covers the driver's 50 ms deadtime and the start-up surge at every start, reversal, stop and resume |
| `current_max_ma` | 1000 scaled | One reading over it stops the drive at once, even in the surge window, with no reversal |
| `reverse_ms` | 1000 | About 13 degrees of rotor back, a ninth of a compartment: meant to unseat a kibble without dispensing. Provisional |
| `direction_pause_ms` | 500 | The stop between directions |
| `max_reverse_attempts` | 3 | Stock's count |

Notes:

- **Measured current.** A drive reads a mean of 59–64 scaled and peaks of 164–168 empty, a mean of 82–87 with food, and 15–16 stopped. A start-up surge read 333 on one poll 17 ms in, and up to 447 at 10 ms polling. Full scale reads about 3192, so 1000 is a third of the channel's range and more than twice the highest surge.
- **Units.** Milliamps need a reference. R36 (1.00 Ω) is the current sense, so the scaled values are about milliamps if the scale is pin millivolts. That is still to be confirmed.
- **Reversal length.** Stock's reversal length is not recovered, so 1 s stays provisional until a real jam shows what frees it. A turn is about 27 s of drive.

**Threshold risk.** The 300 and 1000 come from empty and light-food drives on the adapter. A full hopper's peaks, peaks on batteries and a real stall's current are not measured. Peaks near 300 would make sound feeds back off, and end them after three reversals. A stall reading over 1000 would stop every jam at once with no reversal: safe, but never freeing one.

Rules the controller keeps (host-tested in `wellfed-core`'s `feed_controller`):

- **Bounded time.** Each reversal adds what it costs, 2.5 s (the time back, the pause, the time forward to win the ground back), to the feed's deadline and to its feedback window. Three reversals add at most 7.5 s, and the deadline never passes `max_duration_ms`. A jam that clears in time completes its portions. A slower rotor or a jam freed late can still end the feed on its deadline.
- **A jam both ways ends it.** The limit held while backing off, or during the pause, ends the feed at once, with no more motion either way.
- **A start refused over the limit.** Current over the limit with the motor stopped is a faulty reading, so the feed does not start (`Sensor`, below).

**Each mark counted once across a reversal.** The 5 s inhibit window stays anchored to the last credited portion (or the start), not to the resume. The rotor wins its 1 s back in about a second, so a window restarted there would hide a real mark just past the jam. About 44 % of jam positions would lose a mark, and under-report food.

- Edges captured before the resume are dropped.
- The one boundary backing off can recross is the rise the rotor passed just before the jam. If the last forward edge was a rise (credited or inside the inhibit window) and PA18 read LOW while backing off, the next forward rise is that same one and is skipped, with a fresh inhibit window for its bounces.
- Backing off (1 s) is shorter than a mark (1.2–1.4 s), so at most one boundary is recrossed.
- A double bowl's first direction counts the fall into a mark instead, so there the recross is the input reading HIGH while backing off. Any non-boundary edge first (the rotor never left the mark) cancels the skip (`a_freed_jam_in_a_double_bowl_d1_credits_each_fall_once`).
- `a_freed_jam_anywhere_in_a_mark_period_credits_each_mark_once` sweeps jam positions over two mark periods with one to three reversals. The simulator's `a_jam_next_to_a_mark_credits_each_mark_once` puts a jam just before a mark and just past one.
- If a recross's LOW lasts less than one 20 ms poll, it is not seen, and the inhibit window covers it only for a single reversal (recovered).

Stock counts a feed ended by a jam as done (record 2031). Wellfed records it failed, with what it completed: it never claims food it cannot count.

## Missing or stuck feedback

The rotor's mark sensor (PA18) rests HIGH and reads LOW for 1.2–1.4 s at each mark, one mark a portion. Marks come 9.1 s of drive apart empty and 9.8 s with food, so a turning rotor shows an edge at least every 8.5 s.

| Fault | Detection | Ends as |
| --- | --- | --- |
| Marks lost: sensor gone, PA18 stuck HIGH, or a rotor stalled with no current to show it | No edge of either level for `mark_gap_ms` (11 s) of forward drive, counted afresh at each resume; or no portion within the 15 s feedback timeout | `Failed`, `FeedbackTimeout` (`NoFeedback` on the wire) |
| Mark input stuck LOW | PA18 LOW for `mark_low_ms` (4 s) of forward drive; a reversal or pause restarts the count | `Failed`, `Sensor` |

Missing feedback is never a reason to reverse. Without marks the controller cannot tell a jammed rotor from a lost sensor, and backing off blind is motion it cannot account for. A one-portion feed with no marks ends at 11 s, inside its 12 s deadline, so the reason says what went wrong rather than `Timeout`.

The level comes from the owner's own acquisitions (one port read with PA16, PA17 and PA19), not from the edge interrupts, so a stuck input with no edges is still seen.

## Outlet and food sensors

**Blocked outlet.** A beam blocked for 1 s (the stock filter) during a drive ends the feed `Failed`, `Outlet`.

**A feed that never starts** because of an input ends with that input's reason. A blocked outlet, an unsettled or unreadable beam, low power, or current over the limit at rest keeps it from starting until its 2 s prepare window runs out. It then ends `Failed` with `Outlet`, `Sensor` or `Power`, with no motion (`motion_possible` false). A feed that only ran out of time still ends `Expired`, `TotalTimeout`: the journal's format holds an expired result to that reason, and bank 0 reads it with the same rule, so a reason of its own takes the failed outcome.

Stock pauses a feed on a blocked outlet and resumes within about 10 min (record 2008). Wellfed ends it. Whether to wait and retry is owner-visible policy.

**Food beam.** The owner reads the food beam only while a feed's emitters are on, and publishes its latest qualified reading (`OwnerStatus::food`), which outlasts the feed. Food shows at once, and empty only after 1 s of readings without: stock's `surplusGrain` filter, 20 readings at its nominal 50 ms poll. `Feeds::food()` returns the reading, so the status's `food`, the `Food` change event and the statistics' Food section (present, emptied, refilled, feeds since refill) follow it as feeds read it. `present` comes from the first feed after a boot. An emptied or refilled hopper shows as the reading changes between feeds, so a refill between two feeds shows at the second.

Each feed's trace also keeps what the beam read during that feed (`FeedTrace::hopper`: empty once it read empty), which its `Fed` event carries as `hopper` and `PLOWN1 fed … hopper=` logs. A feed never waits on the beam and is never refused for it, as stock's: a beam stuck empty, or a hopper that really is empty, still feeds, and the history says the hopper read empty. A food beam stuck either way reads as a hopper that never changes.

**Implausible beams.** A receiver that reads lit with its emitter off would mean a stuck beam. The owner does not check that yet: the fake hardware does not model receivers that follow their emitters, and ambient light on the receivers is not measured. A stuck-clear outlet beam is the risk it would catch, a blocked outlet read as clear; a jam then still ends the drive.

## Lost sensors: held, not reset on

An ADC conversion that never completes trips the board. The owner then holds the fault rather than resetting:

| Where | What happens |
| --- | --- |
| At rest | Fault `Sensor` (code 7), held. The owner verifies STOP and the emitters off on every pass and keeps beating, so the platform stays up, serves the device API and OTA, and reports `Blocked::Fault`, a lasting reason recorded as `FeedingBlocked`. A restart clears it: the device API's `Reboot`, power, or the one recovery restart below |
| During a feed | The feed fails `Sensor`, durably, then the same hold. A feed cancelled in the same acquisition that lost the sensors also records `Failed`, `Sensor`, since the journal holds a failed result to a fault's reason |

Before this, the owner faulted `Hardware` and stopped beating, so the runtime withheld the watchdog and the board reset. On a converter that stays lost, every boot repeated it: a reset loop every few seconds on a confirmed build.

Why hold rather than reset: on a confirmed build, a reset only meets the same converter again. A trial build is different. Its owner must prove it can feed before the build confirms, and a faulted owner reports `Feeding::Failed`. So the runtime resets the trial at once, KM0 counts it, and the update falls back to bank 0. The hold only ever lasts on a confirmed build. The conversion's bound counts readiness polls, not time, so preemption cannot exhaust it: only a converter that stopped converting fails it.

**One recovery restart.** A held feeder never reads the ADC again, so on its own it would never feed again. After the owner has held lost sensors at rest for 10 min (`RECOVERY_HOLD_MS`), with no feed in flight and no install running, the loop logs `PLOWN1 recover restart=1 held_ms=` and asks for a restart once.

- It goes through the deliberate-restart path every reboot takes (`request_reset`), which settles the trial register as asked-for, so it does not count toward a reset storm.
- It happens only from the first boot of a power cycle. If the sensors are lost again after it, the owner holds them for good, until a `Reboot` or a power cut.
- The boot is told from the KM4 boot count in backup register 3, which is incremented at each arming and cleared by power loss. It is read and never written: no backup register bit is free to count recovery restarts (1 is rewritten at every arming, 2 is the crumb, 3 and 4 the cores' boot counts and the trial, 5 KM0's heartbeat). So a power cycle that already had any reset, such as an update or a watchdog reset, gives up its recovery restart.
- While a reset storm holds the feeder (`wellfed_sdk::held`), the owner stays closed and the loop never reaches the recovery.

Only lost sensors are held. A lost supervisor (the timer, its clock, the owner's lease with the emitters on) or a board faulted by an output failure still lapses the beat into a watchdog reset. The supervision itself is in doubt there, and the reset is the independent path ([motor and feeds](motor-pulse.md#a-stalled-loop-and-a-failed-stop-during-a-feed)).

## Records

- `PLOWN1 fed … samples=N reversals=N`: the reversals the drive made.
- `PLOWN1 state=7 fault=7`: the owner holds lost sensors.
- `PLOWN1 recover restart=1 held_ms=`: the one recovery restart, about to happen.

No format changes. The journal's reasons and outcomes are ones bank 0 already decodes (`Overcurrent`, `FeedbackTimeout`, `Sensor`, `Outlet`, `Power` with `Failed`), and the device API's types are unchanged. Owner fault 7 appears only in UART records and the simulator's reports.

## Fault-injection variants

Two opt-in variants of `feeder`, never in the normal image: `feed-jam` and `sensor-fault`. They enable `wellfed-hal`'s `feed-faults` feature, without which nothing below is compiled. While a fault is armed, the board's input capture reads what that fault would show: the current channel's samples, the GPIO word, an ADC conversion, or the feedback interrupt. The motor really turns and the marks really pass, and everything after the capture is the product's own handling. The pin watch samples the pads directly, so a capture still shows PA18 toggling under a masked sensor, which tells an injected fault from a real one.

The requested portions choose the case (`feed_faults::Planner`, host-tested):

| Variant | Portions | Shown | Expected, empty hopper |
| --- | --- | --- | --- |
| `feed-jam` | 1 | A jam the first reversal frees, 2 s into the drive (forward reads 1000 raw, about 620 scaled, until the motor first drives back) | One reversal, then `Done`, `completed=1`, `reversals=1`, about 11.5 s of drive |
| `feed-jam` | 2 | A jam no reversal frees, 2 s into the drive | Three reversals, then `Failed`, `Overcurrent`, `reversals=3`, about 7 s from the start |
| `feed-jam` | 3 | The mark input stuck LOW, 2 s into the drive | `Failed`, `Sensor`, about 6 s of drive |
| `sensor-fault` | 1 | Marks lost from the drive's start (PA18 reads HIGH, its interrupts dropped) | `Failed`, `FeedbackTimeout`, at about 11 s |
| `sensor-fault` | 2 | The outlet blocked before the feed starts | `Failed`, `Outlet`, no motion, about 2 s |
| `sensor-fault` | 3 | The ADC lost 2 s into the drive | `Failed`, `Sensor`; owner fault 7, readiness `Fault`, no reset until a `Reboot` |

Records: `PLINJ1 show id= fault=` when a feed is shown its fault, with the `Injected` code (1 jam that frees, 2 jam that holds, 3 marks missing, 4 mark stuck LOW, 5 outlet blocked, 6 ADC lost). `PLINJ1 clear id= armed= readings= edges=` follows once that feed has ended: whether the fault is still armed (0 once a freed jam cleared itself), the captures it changed, and the feedback interrupts it dropped.

> [!WARNING]
> Every feed is shown its fault, whatever starts it. Never leave a variant installed.

Host coverage: `wellfed-hal`'s `tests/feed_faults.rs` (built with `--features feed-faults`; `make check-rust` runs it) runs each case through the owner. The simulator models the same faults on the measured plant (`wedge` for a jam that frees or holds, `jam` with `edge` for lost or stuck marks, `inputs` for the outlet, `adc_stuck` for the ADC), in `wellfed-sim`'s `virtual_feeder` tests and the `jam-freed` and `jam-held` scenarios under `src/firmware/vendor/sim/scenarios`.

## Rerunning the check

Unattended, hopper empty, D cells out, at most 3 portions a feed, the logic analyzer at the motor (D2, D3) recording all eight channels at 1 MHz. About an hour: three over-the-air installs to bank 1 (no bank-0 write) and six feeds, the longest driving about 12 s.

1. Run `wellfed-bench status`. You should see relay 1 on, UART relays closed and no capture or server running, with the hotspot up and the normal `feeder` confirmed on bank 1. Note the journal's high water from `wellfed status`.
2. Run `wellfed-bench ota NAME --build --firmware feeder --variant feed-jam` and wait for it to confirm. Run no feed before the confirmation.
3. For each feed, start a 180 s UART capture and the analyzer for 60 s, then run `wellfed feed --portions N --address …`, and afterwards `wellfed events` for the `Fed` event.
4. Feed `feed-jam` with 1, 2 and 3 portions. Check the outcome, reason and `reversals=` against the table. On the analyzer you should see forward (D3 HIGH), back (D2 HIGH) for 1.0 s and stopped 0.5 s, plus the 50 ms deadtime each way.
5. Install `sensor-fault`, wait for the confirmation, then feed 1, 2 and 3 portions.
   - For 1, `PLOWN1 pins` shows PA18 toggling (the real marks) while `PLINJ1 clear … edges=` counts the dropped interrupts.
   - For 2, the analyzer shows no drive.
   - For 3, after the `Fed` event, `wellfed status` reports readiness blocked `Fault`, `wellfed events` a `FeedingBlocked`, and the UART no `PLSUP1 stop` for 2 minutes. Then run `wellfed reboot`: the next boot is Idle and ready with `FeedingBlocked` cleared. The variant came in by an over-the-air restart, so this power cycle has had a reset and its recovery restart stays off: the hold must last past 10 min if left.
6. Install the normal `feeder` (`--build --firmware feeder`), wait for its confirmation, check `wellfed-bench status`, and leave the hotspot up with no capture or server running.

> [!WARNING]
> Stop condition: a drive at the motor 1 s past its expected end, or any drive after a reset, means power off (`wellfed-bench off`, or `boot`), then stop and report.

**Stop timing under motion.** The independent stop while the owner stalls is measured: the TIM2 lease ended both stalled drives 0.18–0.2 s after the stall began ([motor and feeds](motor-pulse.md#evidence)). This check measures the controller's own stop. In the 2-portion jam, every forward drive after a pause reads the jam from its first sample, so its length at the motor less the 150 ms surge window is the time from detection to the drive's end: two 20 ms polls and the step that applies STOP, expected 190–250 ms in all. The reversal and pause lengths check the controller's timing against the analyzer's clock. In the ADC case the drive ends within one poll (about 20 ms) of the variant's arming, which the UART's `PLINJ1 show` dates only to the host's timestamps.

**Watchdog clock bounds.** The bounds stay 24–54 kHz. Nothing in these stops depends on the watchdog, since the controller and the TIM2 lease end every drive, and the wide bounds only make the fault variants' ceilings more conservative. Narrowing them would change the audited supervision constants for no stop that needs it.

## Left for the owner

- **A real jam.** A kibble or a wedge in the rotor, with food. The stall current the 300 limit is meant to catch is not measured (the simulator's 1000 raw is a guess), nor whether a 1 s reversal frees a real jam.
- **Taped sensors.** PA18 covered (lost marks), a stuck mark, the outlet beam blocked with paper before and during a feed, and the food beam covered and uncovered between feeds.
- **The rotor on D cells.** A feed on batteries alone with the analyzer and UART. The gap between marks (8.5 s at most on the adapter) must stay well under the 11 s `mark_gap_ms`, and the drive current under the 300 limit, or both limits need a battery case.
- **A current reference.** A USB power meter in series, or a confirmed sense-resistor scaling, to put 300, 1000 and the surge in milliamps.
- **Food readings.** The food beam's reading across a refill, so the statistics' emptied and refilled counts have a physical check.

## Evidence

- **The variants on hardware.** Observed (October 26, `feed-jam` v233 and `sensor-fault` v234, hopper empty, feeds 48–53, UART captures `fw19-{jam,sensor}-20261026-feed{1,2,3}-uart`):
  - A jam that frees credited its portion after one reversal (feed 48, `Done`, 11.5 s).
  - A jam that persists ended `Failed`, `Overcurrent` after three reversals (49, 7.2 s).
  - A mark input stuck LOW ended `Sensor` (50, 6.1 s, no reversal).
  - Dropped mark interrupts ended `NoFeedback` at 11.0 s while the pin watch saw PA18 toggle (51).
  - A blocked outlet refused with no drive (52).
  - A lost ADC held owner fault 7 with readiness blocked `Fault` and no reset in 180 s, cleared by a reboot (53).
  - Every feed ended stopped and durable, with no reset.
- **In a long run.** Observed (October 27, the first sustained bench run): the `feed-jam` cases repeated as above (Done after one reversal, Overcurrent after three, Sensor). The `sensor-fault` cases gave `FeedbackTimeout`, `Outlet` with no drive, and `Sensor` with the owner going to its fault. The owner logs that fault state as the supervisor stops the drive, before the feed's result.
- **Current figures.** Observed: drive means and peaks, the 333 surge on v166 and the 447 surge on the first pulse ([motor and feeds](motor-pulse.md#evidence)).
- **Stock.** Recovered: stock reverses over 300 and ends the feed after the third reversal; it counts a jam-ended feed as done (record 2031) and pauses on a blocked outlet (record 2008).
- **Watchdog clock.** Observed: nine watchdog resets on October 25 and 26 came 3.97–4.05 s after the last refresh, about 32.4–32.8 kHz, the 32.768 kHz SDM clock. The supervision probe's 43 kHz was most likely its 1 s stall reporting (recovered).
- **Host and simulation.** Modeled: the controller rules above, and every jam position swept, single and double bowl (45,360 + 490 simulated runs clean).

# Jams, missing feedback and sensor faults

Status: host-tested and simulated (October 26); not yet run on hardware. The bench plan is [below](#bench-plan); real jams, taped sensors and a current reference wait for the owner ([left for the owner](#left-for-the-owner)).

This is what the feeding owner does when a feed meets a jam, loses its feedback or its sensors, and what a feed refused by an input says. Every case ends with the motor stopped, an explicit outcome and reason in the journal and the `Fed` event, and no automatic restart: a client never resends an uncertain feed, and the feeder never re-runs one. The limits are in `feeding::CONFIG` (`src/firmware/wellfed/feeder/src/feeding.rs`); the behaviour is the controller's (`wellfed_core::feed::controller`) and the owner's (`wellfed_hal::owner`). The drive itself, its marks and its measured current are in [motor pulse](motor-pulse.md#feeds-through-the-owner).

## Jams: bounded reversals

A jam is the motor current held over the limit: two consecutive polls (20 ms apart) over 300 scaled, outside the 150 ms after any change of drive. The controller then backs off: 1 s the other way, a 0.5 s stop, and forward again. At most three times, as stock does (static: stock reverses over 300 and ends the feed after the third reversal). After the third, the next jam ends the feed `Failed`, `Overcurrent`.

| Limit | Value | Why |
| --- | --- | --- |
| `current_limit_ma` | 300 scaled | Stock's limit, in stock's unit (milliamps need a reference). A drive reads a mean of 59–64 and peaks of 164–168 empty, a mean of 82–87 with food, and 15–16 stopped (physical, FW16): the limit is 1.8 times the highest peak seen |
| `surge_ms` | 150 | A start-up surge read 333 on one poll 17 ms in (v166) and up to 447 at 10 ms polling (the first pulse). The window covers the driver's 50 ms deadtime before the drive applies, and the surge after it, at every start, reversal, stop and resume |
| `current_max_ma` | 1000 scaled | One reading over it stops the drive at once, in the surge window too, with no reversal: more than twice the highest surge seen, and a third of the channel's range (full scale reads about 3192) |
| `reverse_ms`, `direction_pause_ms` | 1000, 500 | About 13 degrees of rotor back (a turn is about 27 s of drive, three compartments), a ninth of a compartment: meant to unseat a kibble without dispensing (untested on a real jam) |
| `max_reverse_attempts` | 3 | Stock's count |

Rules the controller keeps (host-tested in `wellfed-core`'s `feed_controller`):
- **Bounded time.** Each reversal adds what it costs, 2.5 s (the time back, the pause, the time forward to win the ground back), to the feed's deadline and to its feedback window, so a jam that clears does not then time out. Three reversals add at most 7.5 s, and the deadline never passes `max_duration_ms`.
- **A jam both ways ends it.** The limit held while backing off, or during the pause, ends the feed at once: no more motion either way.
- **Edges while backing off never count.** The feedback queue credits an edge only while the gate applies exactly the commanded forward drive, and the inhibit window restarts at each resume.
- **A start refused over the limit.** Current over the limit with the motor stopped is a faulty reading: the feed does not start (`Sensor`, below).

Stock counts a feed ended by a jam as done (record 2031). The replacement records it failed, with what it completed: it never claims food it cannot count.

## Missing or stuck feedback

The rotor's mark sensor (PA18) rests HIGH and reads LOW for 1.2–1.4 s at each mark, one mark a portion, 9.1 s of drive apart empty and 9.8 s with food. A turning rotor shows an edge at least every 8.5 s.

| Fault | Detection | Ends as |
| --- | --- | --- |
| Marks lost (sensor gone, PA18 stuck HIGH, or a rotor stalled with no current to show it) | No edge of either level for `mark_gap_ms` (11 s) of forward drive, counted afresh at each resume; or no portion within the 15 s feedback timeout | `Failed`, `FeedbackTimeout` (`NoFeedback` on the wire) |
| Mark input stuck LOW | PA18 read LOW for `mark_low_ms` (4 s) of forward drive; a reversal or pause restarts the count | `Failed`, `Sensor` |

Missing feedback is never a reason to reverse: without marks the controller cannot tell a jammed rotor from a lost sensor, and backing off blind is motion it cannot account for. A one-portion feed with no marks ends at 11 s, inside its 12 s deadline, so the reason says what went wrong rather than `Timeout`.

The level comes from the owner's own acquisitions (one port read with PA16, PA17 and PA19), not from the edge interrupts, so a stuck input with no edges is still seen.

## Outlet and food sensors

**Blocked outlet.** Already in FW16: a beam blocked for 1 s (the stock filter) during a drive ends the feed `Failed`, `Outlet`. New: a feed that never starts because of an input ends with that input's reason. A blocked outlet, an unsettled or unreadable beam, low power, or current over the limit at rest kept it from starting until its 2 s prepare window ran out, so it ends `Failed` with `Outlet`, `Sensor` or `Power`, with no motion (`motion_possible` false). A feed that only ran out of time still ends `Expired`, `TotalTimeout`: the journal's format holds an expired result to that reason (bank 0 reads it with the same rule), so a reason of its own takes the failed outcome. Stock pauses a feed on a blocked outlet and resumes within about 10 min (record 2008); the replacement ends it, and whether to wait and retry is FP08's owner-visible policy.

**Food beam.** The owner reads the food beam only while a feed's emitters are on (their duty is FW30's), and publishes its latest qualified reading (`OwnerStatus::food`), which outlasts the feed. `Feeds::food()` returns it, so the statistics' Food section (present, emptied, refilled, feeds since refill) fills as feeds read it: `present` from the first feed after a boot, and an emptied or refilled hopper as the reading changes between feeds (a refill between two feeds shows at the second). A food beam stuck either way reads as a hopper that never changes; it gates no feed.

**Implausible beams.** A receiver that reads lit with its emitter off would show a stuck beam. The owner does not check that yet: the fake hardware does not model receivers that follow their emitters, and ambient light on the receivers has not been measured. A stuck-clear outlet beam is the risk it would catch (a blocked outlet read as clear); a jam then still ends the drive.

## Lost sensors: held, not reset on

An ADC conversion that never completes trips the board, as before. What changed is what the owner does next:

| Where | Before | Now |
| --- | --- | --- |
| At rest | Fault `Hardware`; the owner never beats again, so the runtime withholds the watchdog and the board resets. On a converter that stays lost, every boot repeats it: a reset loop every few seconds on a confirmed build | Fault `Sensor` (code 7), held: the owner verifies STOP and the emitters off on every pass and beats on, so the platform stays up, serves the device API and OTA, and reports `Blocked::Fault` (a lasting reason, recorded as `FeedingBlocked`). A restart (the device API's `Reboot`, or power) clears it |
| During a feed | The feed fails `Output`, then the same reset | The feed fails `Sensor`, durably, then the same hold |

Why hold rather than retry or reset: the owner opens only once the flash hooks are armed, which comes after the build confirmed itself, so a reset never protects a trial; on a confirmed build it only meets the same converter again. The conversion's bound counts readiness polls, not time, so preemption cannot exhaust it: only a converter that stopped converting fails it, which a retry in place would not fix either. Held, the owner can see why the feeder cannot feed and restart it remotely. The cost: a transient converter fault would need that restart. An automatic restart once per power cycle could cover it; that is the reset-storm policy FW61 owns.

Only lost sensors are held. A lost supervisor (the timer, its clock, the owner's lease with the emitters on) or a board faulted by an output failure still lapses the beat into a watchdog reset, as FW16 showed on hardware: the supervision itself is in doubt there, and the reset is the independent path.

## Records

- `PLOWN1 fed … samples=N reversals=N`: the reversals the drive made, appended to FW16's record.
- `PLOWN1 state=7 fault=7`: the owner holds lost sensors.

## Storage and protocol

No format changes. The journal's reasons and outcomes are the ones bank 0 already decodes (`Overcurrent`, `FeedbackTimeout`, `Sensor`, `Outlet`, `Power` with `Failed`), and the device API's types are unchanged. Owner fault 7 appears only in UART records and the simulator's reports.

## Fault-injection variants

Two opt-in variants of `feeder`, never in the normal image: `feed-jam` and `sensor-fault` enable `wellfed-hal`'s `feed-faults` feature, without which nothing below is compiled. While a fault is armed, the board's input capture reads what that fault would show (the current channel's samples, the GPIO word, an ADC conversion, or the feedback interrupt); the motor really turns and the marks really pass, and everything after the capture is the product's own handling. The pin watch samples the pads directly, so a capture still shows PA18 toggling under a masked sensor, which tells an injected fault from a real one. The requested portions choose the case (`feed_faults::Planner`, host-tested):

| Variant | Portions | Shown | Expected (empty hopper) |
| --- | --- | --- | --- |
| `feed-jam` | 1 | A jam the first reversal frees, 2 s into the drive (forward reads 1000 raw, about 620 scaled, until the motor first drives back) | One reversal, then `Done`, `completed=1`, `reversals=1`, about 11.5 s of drive |
| `feed-jam` | 2 | A jam no reversal frees, 2 s into the drive (every forward drive reads it) | Three reversals, then `Failed`, `Overcurrent`, `reversals=3`; about 7 s from the start |
| `feed-jam` | 3 | The mark input stuck LOW, 2 s into the drive | `Failed`, `Sensor`, about 6 s of drive |
| `sensor-fault` | 1 | The marks lost from the drive's start (PA18 reads HIGH, its interrupts dropped) | `Failed`, `FeedbackTimeout`, at about 11 s |
| `sensor-fault` | 2 | The outlet blocked before the feed starts | `Failed`, `Outlet`, no motion, about 2 s |
| `sensor-fault` | 3 | The ADC lost 2 s into the drive | `Failed`, `Sensor`; the owner holds fault 7, readiness `Fault`, no reset until a `Reboot` |

Records: `PLINJ1 show id= fault=` when a feed is shown its fault (the `Injected` code: 1 jam that frees, 2 jam that holds, 3 marks missing, 4 mark stuck LOW, 5 outlet blocked, 6 ADC lost), and `PLINJ1 clear id= armed= readings= edges=` once that feed has ended: the fault still armed (0 once a freed jam cleared itself), the captures it changed, and the feedback interrupts it dropped. Every feed is shown its fault, whatever starts it, so never leave a variant installed.

Host coverage: `wellfed-hal`'s `tests/feed_faults.rs` (built with `--features feed-faults`; `make check-rust` runs it) runs each case through the owner. The simulator models the same faults on the measured plant (`wedge` for a jam that frees or holds, `jam` with `edge` for lost or stuck marks, `inputs` for the outlet, `adc_stuck` for the ADC): `wellfed-sim`'s `virtual_feeder` tests, and the `jam-freed` and `jam-held` scenarios under `src/firmware/vendor/sim/scenarios`.

## Bench plan

Unattended, hopper empty, D cells out, at most 3 portions a feed, the logic analyzer at the motor (D2, D3) recording all eight channels at 1 MHz. About an hour: three over-the-air installs to bank 1 (no bank-0 write) and six feeds, the longest driving about 12 s.

1. **Before.** `wellfed-bench status`: relay 1 on, UART relays closed, no capture or server running; the hotspot up; the feeder answers with the normal `feeder` confirmed on bank 1. Note the journal's high water from `wellfed status`.
2. **Install `feed-jam`** (`wellfed-bench ota NAME --build --firmware feeder --variant feed-jam`) and wait for it to confirm. No feed before the confirmation.
3. **Each feed:** a 180 s UART capture, the analyzer for 60 s, then `wellfed feed --portions N --address …`, and afterwards `wellfed events` for the `Fed` event.
   - **Stop condition:** a drive at the motor 1 s past its expected end (the table above), or any drive after a reset: power off (`wellfed-bench off`, or `boot`), then stop and report.
4. **`feed-jam`, 1, 2 and 3 portions.** Check the outcome, reason and `reversals=` against the table, and on the analyzer the drive's segments: forward (D3 HIGH), back (D2 HIGH) for 1.0 s, stopped 0.5 s (plus the 50 ms deadtime each way).
5. **Install `sensor-fault`**, wait for the confirmation, then 1, 2 and 3 portions. For 1, `PLOWN1 pins` must show PA18 toggling (the real marks) while `PLINJ1 clear … edges=` counts the dropped interrupts. For 2, the analyzer must show no drive. For 3, after the `Fed` event: `wellfed status` reports readiness blocked `Fault`, `wellfed events` a `FeedingBlocked`, and the UART no `PLSUP1 stop` for 2 minutes; then `wellfed reboot`, and the next boot is Idle and ready with `FeedingBlocked` cleared.
6. **Restore** the normal `feeder` (`--build --firmware feeder`), wait for its confirmation, check `wellfed-bench status`, and leave the hotspot up with no capture or server running.

**Stop timing under supervised motion** (the carried item). The independent stop while the owner stalls was measured in FW16: the TIM2 lease ended both stalled drives 0.18–0.2 s after the stall began ([result](motor-pulse.md#result-october-26)). The controller's own stop is measured here, from segments in which the fault is present from their first sample: in the 2-portion jam, every forward drive after a pause reads the jam from its start, so its length at the motor, less the 150 ms surge window, is the time from the jam's detection to the drive's end (two 20 ms polls and the step that applies STOP; expected 190–250 ms in all). The reversal and pause lengths check the controller's timing against the analyzer's clock. The ADC case shows a stop from an acquisition failure: its drive ends within one poll (about 20 ms) of the variant's arming, which the UART's `PLINJ1 show` dates only to the host's timestamps.

**Watchdog clock bounds** (the other carried item). Nine watchdog resets on October 25 and 26 came 3.97–4.05 s after the last refresh, about 32.4–32.8 kHz, the 32.768 kHz SDM clock (physical); FW05's 43 kHz was likely its probe's 1 s stall reporting (static). The bounds stay 24–54 kHz: nothing in this card's stops depends on the watchdog (the controller and the TIM2 lease end every drive above), and the wide bounds only make the ceilings the fault variants are sized against more conservative. Narrowing them would change the audited supervision constants for no stop that needs it.

## Left for the owner

- **A real jam.** A kibble or a wedge in the rotor, with food: the stall current the 300 limit is meant to catch is not measured (the simulator's 1000 raw is a guess), nor whether a 1 s reversal frees a real jam.
- **Taped sensors.** PA18's mark sensor covered (lost marks, physical), a stuck mark, the outlet beam blocked with paper before and during a feed, the food beam covered and uncovered between feeds.
- **A current reference.** A USB power meter in series, or the sense resistor's value off the board, to put the scaled thresholds (300, 1000, the 150 ms surge) in milliamps.
- **Food readings.** The food beam's reading across a refill, so the statistics' emptied and refilled counts have a physical check.

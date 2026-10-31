# Portions, phase and calibration

The feeder counts portions, not grams. A portion is one compartment of the rotor, and the feeder knows one has dropped when the rotor's mark sensor sees it pass. What a portion weighs comes from your kitchen scale, entered once with `wellfed calibrate`. This page explains what a portion is, how the feeder copes when the rotor stopped between marks, and how to weigh a portion.

Status: **Works**. Phase handling is observed on our unit, and one portion of the owner's dry food weighs 7.11 g on it.

## What a portion is

The rotor has three compartments over one drop hole, and a portion is one compartment: a third of a turn. Each compartment fills from the hopper and empties by gravity over the hole.

The mark sensor (PA18) rests HIGH and reads LOW for about 1.2–1.4 s at each compartment's mark. The owner credits a portion on the mark's rise, which comes about 9.1 s of drive apart when empty and 9.8 s with food. It stops the motor about 20 ms after the rise, so a completed feed leaves the rotor just past a mark.

The feeder has no scale. It knows how many marks passed, never what fell or what the pet ate ([product architecture](product-architecture.md)). Delivered food is an estimate: credited portions times your measured weight of one portion.

## Phase

The phase is where the rotor rests between feeds (`RotorPhase`, `wellfed_core::feed::storage`):

- **At a mark.** The last feed ended `Done`, on its last portion's rise.
- **Unknown.** The last feed that may have moved the motor ended any other way: cancelled, failed, or uncertain after a reset. The rotor may be anywhere between two marks, including inside one (PA18 LOW).

A feed that never moved the motor leaves the phase as it was.

The phase is never stored. The owner reads it at boot from the feed journal's last result (`OwnerStorage::last_result`). A journal with no result yet reads as unknown: a new install, or a unit that ran stock, which leaves the rotor wherever it stopped. The journal's record layout is unchanged, so an older build in bank 0 reads the same journal.

### What a feed from an unknown phase does

Stock credits a single bowl's portion on the first PA18 rise at least 5 s after the drive starts or after the last portion. It neither homes the rotor nor blocks a feed after an interruption. Wellfed keeps that crediting and adds a homing mark:

- **A rise inside the 5 s inhibit window is the homing mark.** The rotor reached its first mark from wherever it rested, having turned less than about 55 % of a compartment (5 of 9.1 s). It is never credited, and the request's portions count from it, as if the drive had started at a mark. Stock ignores the same rise and credits the next, so without a jam the motor does what stock's does. Wellfed also records the homing: `homed_ms`, the `PLOWN1 marks` line and the portions statistics.
- **A first rise past the window is credited** as a portion, as stock credits it. More than about 55 % of a compartment moved before it.
- **A jam before the homing mark** backs off and drives on as in any feed ([jams and sensor faults](jams-and-sensor-faults.md)). Backing off costs drive time without travel, so while homing the window grows by what the reversals cost, the same amount they add to the deadline. The window still bounds how far the rotor turned, not how long it took. That is where Wellfed parts from stock after a reversal: it homes on a mark that a fixed 5 s window would credit. A boundary recrossed after backing off is skipped before the homing check, so the homing mark is always a new mark (`a_jam_before_the_homing_mark_still_homes_on_it`).
- **The deadline** of a request that may home is one inhibit window (5 s) longer, within the 48-portion cap. Without it, a 1-portion feed from about 4–9 s past a mark would time out before its mark: the homing rise at up to 5 s, then a full compartment of 9.8 s with food, past the 12 s a portion allows. The virtual feeder shows both (`a_feed_after_a_cancel_homes_the_rotor_then_counts_from_the_mark`).

So a feed from an unknown phase drops its requested portions plus whatever was left of the interrupted compartment. That is at most about 55 % of a portion more than it credits when it homes, and about 45 % less when its first credited compartment was partly emptied before. Whether a part-turned compartment holds or drops its food depends on where the hole sits in its turn, which nobody has observed. The [weighing session](#weighing-session) can bound it.

**Why the phase does not block feeds.** Blocking every feed until a homing move would starve a pet after any cancel or power cut, schedules included. The homing move would itself drop food, so it would be a feed under another name. Homing inside the next real feed costs at most one part-compartment, once, and needs no readiness reason. Its window never holds a whole compartment of forward drive: 5 s plus each of three reversals' 1 s return stays under the shortest 9.1 s between marks, which the firmware asserts at compile time (`MARK_PERIOD_MIN_MS` in `feeding.rs`). The phase is reported instead: `Op::GetCalibration`'s `phase`, and the portions statistics' `homed` count.

A double bowl alternates its crediting direction on every second mark, so one mark cannot home an unknown phase. The owner refuses a double-bowl feed with an unknown phase (`SubmitResult::RejectPhase`), and the controller refuses a homing request in double-bowl mode.

## Bowl mode

The PLAF103 has one drop hole and turns clockwise (`Drive0`, PA13 HIGH) to feed. Stock's feeding code also has a double-bowl branch: it alternates direction with each portion and pauses between, and crediting needs two marks per portion. Nothing on this unit has run it, and with one hole the mode has nowhere to send a second bowl's food.

So the bowl mode belongs to the build, not to a setting. `feeding::BOWL` is `Single`, and the feed journal pins it: a journal written in another mode reads as corrupt. The device API reports it (`Calibration::bowl`) and refuses a `PutCalibration` that names another mode with `Refusal::Unverified`. The mode exists on the wire with no way to turn it on. A build verified for a double bowl would need its own journal binding and a bench run of the direction pauses and crediting. The settings record's bowl byte keeps the `Single` it has always held.

## Calibration parameters

| Parameter | Unit | Limits | Default | Stored |
| --- | --- | --- | --- | --- |
| `portion_mg`, the mean weight of one portion | Milligrams | 1,000–50,000 (`PORTION_MG_MIN`, `PORTION_MG_MAX`), or none | None (not measured) | Storage sectors 34–35 |

The limits are sanity bounds, not a fit: a compartment of dry food weighs a few grams to a few tens of grams. A value outside them is refused `Invalid` at the device API, never written by the store, and a record holding one is not read.

Only one weight is kept. The food beam (PA17) says only whether food is present, so the feeder cannot tell a full hopper from a low one, and a level-dependent weight would need a level it does not measure. The weighing session measures both ends. If they differ by more than about 10 %, a later change can key a second weight on `feeds_since_refill`.

The weight never changes how far the motor turns. A portion is a mark, whatever it weighs.

### Storage

A dual-slot store, `PLFCAL01` (`wellfed_feeder::store::calibration`), holds the generation, `portion_mg` at bytes 20–23 (0 for none), zero padding, a CRC and a commit marker, pinned in `store_layout.rs`. Its sectors, 34 and 35 of the 48-sector storage region (`storage::CALIBRATION`), were never allocated before. So a build that predates the store never reads or writes them: the old bank 0 (`feeder` `92c7adec…`) and any older bank 1 ignore it. The settings record, the journal and every other store are unchanged. The settings writer saves it on its write turn, never during a feed, inside a marked attempt like the time settings. The [factory reset](pairing.md#factory-reset) keeps it: the weight belongs to the hardware, not the owner.

### Wire

`Capabilities::CALIBRATION` is bit 256, after `TRACES` (64) and `POWER` (128). The ops, outcome, refusal and statistics section are appended after the traces' codes:

- `Op::GetCalibration` returns `Outcome::Calibration(CalibrationView { generation, writable, calibration: Calibration { portion_mg, bowl }, phase })`. `phase` is `None` from a feeder with no feeding owner (the simulator) or before it opens.
- `Op::PutCalibration { base, calibration }` is for owners only, answered `Done { generation }` once stored. It is refused `Stale` past `base`, `Busy` while the outputs are claimed or a change waits, `Invalid` outside the limits, and `Unverified` for another bowl mode.
- `StatsSection::Portions` returns `Stats::Portions(Portions { portions, homed, outlet_breaks, portion_mg, delivered_mg })`, since this boot: credited portions, feeds that began with a homing mark, outlet-beam breaks, the stored weight, and `portions × portion_mg`.

A session without the bit is refused `Unavailable` for all three, so an older client or feeder never meets a variant it cannot decode. The fixtures in `tests/fixtures/v1.txt` pin the new messages. The server maps `Unverified` to `Forbidden`, as it maps update refusals, so a server session cannot write the calibration.

### Delivered-food estimate

- **Credited portions** (`Fed { completed }`, `Counts::portions`, `Portions::portions`) are one per mark, the estimate's basis.
- **Homing feeds** (`Portions::homed`) may each have dropped up to about half a portion more than they credited.
- **Outlet-beam breaks** (`Portions::outlet_breaks`, and per feed in `PLOWN1 marks … outlet_breaks=`) come from the outlet beam (PA16) sampled every 10 ms during the feed's capture, two changes a break. Food falls as a stream of kibble rather than one lump per compartment, and a passage shorter than 10 ms falls between samples. So this is a lower bound and an activity check, not a count of portions. A run of credited portions with no breaks at all is the signal of an empty hopper or a jam above the rotor.

Each feed ends with `PLOWN1 marks id=N homed_ms=… outlet_breaks=… phase=at_mark|unknown` after its `fed`, `current`, `edges` and `steps` lines.

## Weighing session

You need food, a kitchen scale reading to 0.1 g (1 g works over more portions) and the feeder on its latest build. Each step is a supervised feed through the device API. Keep hands away from the outlet. You can cut power at any time; the feed then reads as uncertain and is never repeated.

1. **Prime.** With the hopper full, run `wellfed feed --portions 1` once and discard what falls. This fills the compartments and leaves the rotor at a mark. `wellfed calibrate` should show `"phase": "at_mark"`.
2. **Full hopper.** Put the empty bowl on the scale and tare it, or note its weight. Run `wellfed feed --portions 5` twice (10 portions). Each feed's `Fed` event should say `completed: 5`. Note the sum of `completed` and the bowl's net weight.
3. **Low hopper.** Feed until the hopper is near empty: the food beam reports `present: false` in `wellfed stats --section food`, or the level is just above the rotor. Then repeat step 2's 10 portions into the tared bowl.
4. **Phase effect (optional).** Run `wellfed feed --portions 3`, then from a second terminal `wellfed cancel-feed N` (the number `wellfed feed` printed) about 5 s into its drive; discard what falls. Then run `wellfed feed --portions 1` into the tared bowl. `PLOWN1 marks` (or `wellfed stats --section portions`) shows the homing, and the weight shows what a homed feed drops against a normal portion.
5. **Enter the result.** Use the full-hopper run, unless the two runs differ by more than about 10 %; then enter their mean and report both. `wellfed calibrate --weighed 98.4 --portions 10` stores the mean, here 9,840 mg.

Other forms: `wellfed calibrate --portion 9.8` enters one portion directly, `--clear` forgets it, and `wellfed calibrate` alone prints the calibration, the phase and the portions statistics.

Record per run the hopper level, each feed's `completed`, `outlet_breaks` and `drive_ms`, and the net weight. A spread of more than about 10 % between runs at the same level means the compartments do not fill evenly, and the estimate is only as good as that spread.

## Still to check

1. After an install, `wellfed calibrate` answers with `"portion_mg": null`, `"bowl": "single"` and a phase.
2. A ten-portion low-hopper run, if the owner wants one: the run-down below already shows portions near 7 g until the last few.
3. Bank 0's role install leaves sectors 34–35 alone (a give-way and an install over the air kept the weight, October 29).

## Evidence

- **Rotor and marks.** Observed (October 25): three compartments over one drop hole (owner photos), marks 9.1 s apart empty and 9.8 s with food, the stop about 20 ms after the rise ([motor and feeds](motor-pulse.md#evidence)). The bench saw 0, 4 and 8 outlet changes in three 1-portion feeds with food. Every owner feed turned clockwise.
- **Stock crediting.** Recovered: ISR `0x0e01a55c` credits the first PA18 rise at least 5 s after the drive starts or after the last portion. The double-bowl branch is recovered too, never run.
- **Phase across a cancel.** Observed (October 26, v236, hopper empty, `phd-*` captures): feed 54 credited 1 and ended `at_mark`; feed 55, cancelled at 6.28 s, left the phase `unknown`; feed 56 homed at 2,848 ms, completed 1 and ended `at_mark`.
- **Phase across a power cut.** Observed (same session): feed 57 was cut by relay 1 mid-drive and never resent. The next boot logged an uncertain `Fed` (outcome 5) and phase `unknown`. Feed 58 homed at 3,301 ms, completed 1 and ended `at_mark`, and the Portions statistics read `homed 1`.
- **A feed after a mid-drive cut.** Observed (October 27, the sustained bench run): the first feed after a cut saw two mark edges for one credited portion, its first edge at 3.1 s against the usual 7.9 s. That is likely the rotor finishing the cut portion; the weighing session should measure it.
- **Weighing.** Observed (October 29, one dry food, hopper refilled between runs): two 5-portion feeds, 5 of 5 each with drives of 48.0 and 48.6 s, dropped 71.1 g, 7.11 g a portion; an earlier 3-portion feed dropped 21.3 g, 7.10 g a portion. Their outlet beam broke 11 and 6 times, and 2 times for the 3 portions, so beam breaks do not count portions. `wellfed calibrate --weighed 71.1 --portions 10` stored 7,110 mg, which read the same after a reboot.
- **Phase effect.** Observed (October 29, hopper below the food beam): a 3-portion feed cancelled at 5.0 s credited nothing and left the phase `unknown`; its drop was discarded. The next 1-portion feed homed at 4.6 s, credited its portion at 12.8 s (a 14.1 s drive) and dropped 6.6 g, 93 % of a portion. So the part-turned compartment added nothing to the homed portion.
- **Low hopper.** Observed (October 29): eight 1-portion feeds from a hand-emptied hopper dropped 1.7, 11.4, 6.5, 7.0, 6.0, 3.0, 1.9 and 0 g while the food beam read low ([optical inputs](optical-inputs.md#evidence)).
- **Host and simulation.** Modeled (October 26): phase from the journal, the homing window and deadline, the compile-time bound (8.0 s of forward drive against the 9.1 s empty mark period), the bowl refusal, the store, the wire messages and the Portions statistics.

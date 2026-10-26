# Portions, phase and calibration

Status: host-tested and simulated (October 26); no feed with this build has run on hardware, and no portion has been weighed yet. The feeder counts portions by the rotor's marks; what a portion weighs comes from the owner's scale, entered with `wellfed calibrate` ([procedure](#weighing-session)).

## What a portion is

Physical (October 25, [motor pulse](motor-pulse.md#food-and-a-full-turn-october-25)): the rotor has three compartments over one drop hole, a portion is one compartment, a third of a turn. PA18 rests HIGH and reads LOW for about 1.2–1.4 s at each compartment's mark; the owner credits a portion on the mark's rise, about 9.1 s of drive apart empty and 9.8 s with food, and stops the motor about 20 ms after it. A completed feed therefore leaves the rotor just past a mark.

The feeder has no scale. It knows how many marks passed, never what fell or what the pet ate ([product architecture](product-architecture.md)). Delivered food is an estimate: credited portions times the owner's measured weight of one portion.

## Phase

Where the rotor rests between feeds (`RotorPhase`, `wellfed_core::feed::storage`):
- **At a mark:** the last feed ended `Done`, on its last portion's rise.
- **Unknown:** the last feed that may have moved the motor ended any other way (cancelled, failed, or uncertain after a reset). The rotor may be anywhere between two marks, including inside one (PA18 LOW).

A feed that never moved the motor leaves the phase as it was. The phase is never stored: the owner reads it at boot from the feed journal's last result (`OwnerStorage::last_result`), and a journal with no result yet (a new install, or a unit that ran stock, which leaves the rotor wherever it stopped) reads as unknown. The journal's record layout is unchanged, so an older build in bank 0 reads the same journal.

### What a feed from an unknown phase does

Stock credits a single bowl's portion on the first PA18 rise at least 5 s after the drive starts or after the last portion (static, ISR `0x0e01a55c`); it neither homes the rotor nor blocks a feed after an interruption. The replacement keeps that crediting and adds a homing mark:

- **A rise inside the 5 s inhibit window** is the **homing mark**: the rotor reached its first mark from wherever it rested, having turned less than about 55 % of a compartment (5 of 9.1 s). It is never credited, and the request's portions count from it, as from a drive that started at a mark. Stock ignores the same rise and credits the next, so the motor does exactly what stock's does; the difference is that the replacement records it (`homed_ms`, the `PLOWN1 marks` line, the portions statistics).
- **A first rise past the window** is credited as a portion, as stock credits it: more than about 55 % of a compartment moved before it.
- **A jam before the homing mark** backs off and drives on as in any feed ([jams and sensor faults](jams-and-sensor-faults.md)). Backing off costs drive time without travel, so while homing the window grows by what the reversals cost (the same amount they add to the deadline); the window still bounds how far the rotor turned, not how long it took. A boundary recrossed after backing off is skipped before the homing check, so the homing mark is always a new mark (host-tested, `a_jam_before_the_homing_mark_still_homes_on_it`).
- **The deadline** of a request that may home is one inhibit window (5 s) longer, within the 48-portion cap. Without it a 1-portion feed from about 4–9 s past a mark would time out before its mark: the homing rise at up to 5 s, then a full compartment of 9.8 s with food, past the 12 s a portion allows. The virtual feeder shows both (`a_feed_after_a_cancel_homes_the_rotor_then_counts_from_the_mark`).

So a feed from an unknown phase drops its requested portions plus whatever was left of the interrupted compartment: at most about 55 % of a portion more than it credits when it homes, and about 45 % less when its first credited compartment was partly emptied before. Whether a part-turned compartment holds or drops its food depends on where the hole sits in its turn, which nobody has observed; the weighing session can bound it ([below](#weighing-session)).

**Why the phase does not block feeds.** Blocking every feed until a homing move would starve a pet after any cancel or power cut (schedules included), and the homing move would itself drop food, so it would be a feed under another name. Homing inside the next real feed costs at most one part-compartment once, keeps stock's motor behaviour, and needs no readiness reason. The phase is reported instead: `Op::GetCalibration`'s `phase`, and the portions statistics' `homed` count.

A double bowl's phase is a different matter: its crediting alternates directions on every second mark, so an unknown phase cannot be homed by one mark. The owner already refuses a double-bowl feed with an unknown phase (`SubmitResult::RejectPhase`), and the controller refuses a homing request in double-bowl mode.

## Bowl mode

The PLAF103 has one drop hole (owner photos, October 25) and turns clockwise (`Drive0`, PA13 high) to feed, as the bench saw in every owner feed. Stock's feeding code also has a double-bowl branch (static): it alternates direction with each portion and pauses between, and crediting needs two marks per portion. Nothing on this unit has run it, and with one hole the mode has nowhere to send a second bowl's food.

So the bowl mode is the build's, not a setting: `feeding::BOWL` is `Single`, and the feed journal pins it (a journal written in another mode reads as corrupt). The device API reports it (`Calibration::bowl`) and refuses a `PutCalibration` that names another mode with `Refusal::Unverified`, so the mode exists on the wire without a way to turn it on. A build verified for a double bowl would need its own journal binding and a bench run of the direction pauses and crediting. The settings record's bowl byte keeps the `Single` it has always held.

## Calibration parameters

| Parameter | Unit | Limits | Default | Stored |
| --- | --- | --- | --- | --- |
| `portion_mg`: mean weight of one portion | milligrams | 1,000–50,000 (`PORTION_MG_MIN`, `PORTION_MG_MAX`), or none | none (not measured) | Storage sectors 34–35 |

The limits are sanity bounds, not a fit: a compartment of dry food weighs a few grams to a few tens of grams. A value outside them is refused `Invalid` at the device API, never written by the store, and a record holding one is not read.

Only one weight is kept. The food beam (PA17) says only whether food is present, so the feeder cannot tell a full hopper from a low one; a level-dependent weight would need a level it does not measure. The weighing session measures both ends; if they differ by more than about 10 %, a later change can key a second weight on `feeds_since_refill`.

The weight never changes how far the motor turns: a portion is a mark, whatever it weighs.

### Storage

A new dual-slot store, `PLFCAL01` (`wellfed_feeder::store::calibration`): generation, `portion_mg` at bytes 20–23 (0 for none), zero padding, CRC and commit marker, pinned in `store_layout.rs`. Its sectors, 34 and 35 of the storage region (`storage::CALIBRATION`, in the 48-sector region), were never allocated before, so a build that predates the store never reads or writes them: the old bank 0 (`feeder` `92c7adec…`) and any older bank 1 ignore it. The settings record, the journal and every other store are unchanged. The settings writer saves it on its write turn, never during a feed, inside a marked attempt like the time settings.

### Wire

`Capabilities::CALIBRATION` (bit 256), after `TRACES` (64) and `POWER` (128); the ops, outcome, refusal and statistics section are appended after the traces' codes:
- `Op::GetCalibration` → `Outcome::Calibration(CalibrationView { generation, writable, calibration: Calibration { portion_mg, bowl }, phase })`. `phase` is `None` from a feeder with no feeding owner (the simulator) or before it opens.
- `Op::PutCalibration { base, calibration }`, owners only, answered `Done { generation }` once stored; `Stale` past `base`, `Busy` while the outputs are claimed or a change waits, `Invalid` outside the limits, `Unverified` for another bowl mode.
- `StatsSection::Portions` → `Stats::Portions(Portions { portions, homed, outlet_breaks, portion_mg, delivered_mg })`, since this boot: credited portions, feeds that began with a homing mark, outlet-beam breaks, the stored weight and `portions × portion_mg`.

A session without the bit is refused `Unavailable` for all three, so an older client or feeder never meets a variant it cannot decode. The fixtures in `tests/fixtures/v1.txt` pin the new messages. The server maps `Unverified` to `Forbidden`, as it maps update refusals: a server session cannot write the calibration.

### Delivered-food estimate and its cross-check

- **Credited portions** (`Fed { completed }`, `Counts::portions`, `Portions::portions`): one per mark, the estimate's basis.
- **Homing feeds** (`Portions::homed`): each may have dropped up to about half a portion more than it credited.
- **Outlet-beam breaks** (`Portions::outlet_breaks`, and per feed in `PLOWN1 marks … outlet_breaks=`): the outlet beam (PA16) sampled every 10 ms during the feed's capture, two changes a break. Food falls as a stream of kibble rather than one lump per compartment, and a passage shorter than 10 ms falls between samples, so this is a lower bound and an activity check (the bench saw 0, 4 and 8 changes in three 1-portion feeds with food), not a count of portions. A run of credited portions with no breaks at all is the signal: an empty hopper or a jam above the rotor.

Each feed ends with `PLOWN1 marks id=N homed_ms=… outlet_breaks=… phase=at_mark|unknown` after its `fed`, `edges` and `steps` lines.

## Weighing session

For the owner, with food, a kitchen scale reading to 0.1 g (1 g works over more portions) and the feeder on its latest build. Each step is a supervised feed through the device API: hands away from the outlet, and the owner can cut power at any time (the feed then reads as uncertain and is never repeated).

1. **Prime.** With the hopper full, `wellfed feed --portions 1` once and discard what falls. This fills the compartments and leaves the rotor at a mark (`wellfed calibrate` shows `"phase": "at_mark"`).
2. **Full hopper.** Put the empty bowl on the scale and tare it (or note its weight). Run `wellfed feed --portions 5` twice (10 portions). Each feed's `Fed` event must say `completed: 5`; note the sum of `completed` and the bowl's net weight.
3. **Low hopper.** Feed until the hopper is near empty (the food beam reports `present: false` in `wellfed stats --section food`, or the level is just above the rotor), then repeat step 2's 10 portions into the tared bowl.
4. **Phase effect (optional).** `wellfed feed --portions 3`, then from a second terminal `wellfed cancel-feed N` (the number `wellfed feed` printed) about 5 s into its drive; discard. Then `wellfed feed --portions 1` into the tared bowl: `PLOWN1 marks` (or `wellfed stats --section portions`) shows the homing, and the weight shows what a homed feed drops against a normal portion.
5. **Enter the result.** Use the full-hopper run unless the two differ by more than about 10 %, in which case enter their mean and report both:

   `wellfed calibrate --weighed 98.4 --portions 10`

   stores the mean (here 9,840 mg). `wellfed calibrate --portion 9.8` enters one portion directly, `--clear` forgets it, and `wellfed calibrate` alone prints the calibration, the phase and the portions statistics.

Record per run: the hopper level, each feed's `completed`, `outlet_breaks` and `drive_ms`, and the net weight. A spread of more than about 10 % between runs at the same level means the compartments do not fill evenly, and the estimate is only as good as that spread.

## Bench plan

No bench step has run with this build. In order:
1. **Install** the build over the air; confirm `wellfed calibrate` answers with `"portion_mg": null`, `"bowl": "single"` and a phase.
2. **Phase across a cancel (hopper empty, owner present):** a 1-portion feed (expect `phase=at_mark`), then a 1-portion feed cancelled about 5–6 s into its drive (expect `phase=unknown`), then a 1-portion feed: expect `homed_ms` about 3–4 s, `outcome=1`, a drive of about 12–13 s, and `phase=at_mark`.
3. **Phase across a power cut:** a feed cut by `wellfed-bench off` mid-drive; after boot expect the uncertain `Fed` event and `phase: unknown`, and the next feed homes.
4. **The weighing session** above, with food.
5. **Calibration across a restart and an update:** the stored weight survives a reboot and an install; bank 0's role install leaves sectors 34–35 alone.

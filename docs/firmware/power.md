# Power: the supply, the battery floor, brownout and sleep

Status: host tests and simulation (October 26, 2026), static analysis where marked; nothing here has run on the pack yet. The inputs themselves are physical (FW14). What the feeder does on batteries as a feature (which radios stay up, whether schedules run, the low-battery indicator) is the feature board's FP14; this page covers what keeps a feed safe and the supply honestly reported.

## Inputs

From FW14 (physical, October 23, three D cells and a meter):
- **Pack:** ADC channel 5 through a divider of about 2.063 (`power::DIVIDER_PER_MILLE`). Fresh cells read 2357 mV at the pin with the adapter (4.85 V on the meter) and 2306 mV without (4.77 V). With no pack the pin reads about 16 mV.
- **Adapter:** PA19, high while the adapter is in. The feeder rode through both transitions without a reset.
- **Who samples:** the runtime's power monitor every 5 s, or, in the `feeder` firmware, the feeding owner on each of its 20 ms polls (it holds the ADC). Both publish the same sample (`wellfed_hal::power`).

## The supply on the wire

`Capabilities::POWER` (bit 128) carries the supply as the feeder judges it, never an unknown passed off as a value:
- **`Supply { adapter, battery, below_floor }`.** `adapter` is `None` without a fresh sample. `battery` is `Absent` (no pack), `Reading { mv }`, `Implausible { mv }` (above 5.5 V, or on the pack alone below 2.5 V, where the board could not be running), `Unread` (no sample: the monitor has not run, its ADC failed, or the firmware does not sample) or `Stale { age_ms }` (older than three monitor periods). `below_floor` is the feeding owner's verdict on the pack's floor, `None` before it has judged (and in a firmware without an owner). The judging is `wellfed_core::supply::classify`, against the board's `power::PACK`.
- **`Metric::Supply`** goes to subscribers with the bit at each power sample (5 s), beside the version 1 `Power` metric, which is unchanged.
- **`EventBody::Supply`** records the adapter coming or going and the pack crossing its floor, at most one a minute (`feeder::supply::EVENT_MS`): changes inside the minute are recorded once it ends, as the supply then stands, so a chattering adapter or a pack hovering at its floor costs one event a minute. A boot's first reading is a baseline, not an event, and a stale sample changes nothing.
- **Readiness** gains `Blocked::Battery` ([below](#the-floor)), refused `Busy`. A session that claims `READINESS` but not `POWER` gets it as `Blocked::Starting` (`Blocked::seen_by`), the reason that also refuses `Busy` and clears by itself; it is not lasting, so it is never a `FeedingBlocked` event (the `Supply` event records it instead).
- **Statistics:** the `Power` section's battery history, least and most take only `Reading` values, so an implausible reading never enters them.

Nothing is stored in a new place: the events go through the existing ring in its existing 16-byte records. Bank 0 runs an older feeder that cannot decode a `Supply` body; it skips such records as a one-event gap, as it does any newer body, so no bank-0 refresh is needed for this change.

## The floor

The pack's floor is `wellfed_hal::power::FLOOR`, in the battery channel's scaled unit (pin millivolts). It is **provisional**: no motor has run on the pack yet (motion with the cells in is outside the run's bench rules), and the regulator's dropout and the SoC's brownout level are unmeasured on this board (the SDK leaves the BOR thresholds at their reset values and documents them only as codes).

| Limit | Pin mV | Pack | Why |
| --- | ---: | ---: | --- |
| `floor` | 1650 | 3.40 V (1.13 V a cell) | Stock's 0 % point, below which stock stops its outputs on battery (static, [FP14](../devices/plaf103/stock/README.md)) |
| `clear` | 1710 | 3.53 V | Stock's 10 % step: once below, a pack must recover by a step before feeding again |
| `cutoff` | 1550 | 3.20 V | A sag this deep leaves the 3.3 V rail's regulator little headroom; stop at once rather than risk a brownout mid-drive |
| `hold_ms` | 300 | | Fifteen 20 ms polls. The start-up surge lasted one poll on the bench (on the adapter); a sag that outlasts the hold is the pack failing under load |

`wellfed_core::supply::Floor` applies them on every acquisition the owner takes, on the adapter too, so the verdict is current when the adapter is pulled:
- a reading at or above `clear` is above the floor;
- between `floor` and `clear` a pack keeps its verdict (a first reading there counts as above);
- under `floor` for `hold_ms` in a row (a failed conversion counts as under), or under `cutoff` at all, it is below.

Where it acts:
- **A running feed** on the pack alone (PA19 low) stops when the pack is no longer judged above the floor, or its conversion fails: the controller's power fault, `Failed`/`Power`, with STOP applied and the result written as any failure. It is never restarted; a client or schedule must ask again.
- **A new feed** on the pack is refused before it reaches the owner: the owner's `Feeding::blocked` answers `Battery` while `OwnerStatus::supply_low` holds, and readiness reports it. The controller's own start check stays behind it (a feed that lost the race ends `Failed`/`Power` without driving, once its prepare window runs out).
- **On the adapter** the floor never refuses or stops a feed.
- **An adapter change during a feed stops it** (`Failed`/`Power`), as before: the change unsettles power for 100 ms (stock's settle), and the feed does not ride across a change of source. The next feed runs on whichever supply is there.

A pack that sags under load and recovers at rest above `clear` will feed again, and may fail again: each feed ends with an explicit result and none is retried, so this costs failed feeds, not motion without a result.

## Brownout

Static analysis and host tests:
- **The cause is reported.** A boot's reset cause comes from the ROM's flags (`BOOT_Reason` bit 18 is brownout); the statistics report it (`ResetCause::Brownout`) and the `Boot` event carries the raw flags. Whether a real brownout on this board sets that bit is unverified.
- **No feed is replayed.** A brownout is a reset: a feed it interrupted is resolved as `Uncertain` by the next boot's journal recovery, under its own id, and never resumed (FW17, physical for a power cut mid-drive).
- **The next feed waits for the adapter.** A boot whose cause is brownout holds feeding as `Blocked::Battery` until the adapter is seen (`feeder::supply::Watch`). At rest the pack recovers, so its reading alone would let the next feed start and sag it into the same brownout: a loop of resets, each leaving a feed uncertain. New cells are a power cycle, a power-on reset, which does not hold. **On the pack alone the hold lasts until a power cycle**: with no adapter to see, a battery-only feeder that browned out stays `Blocked::Battery` until its cells are taken out (or the adapter is fitted). And since the brownout bit may be stale (below), a later reset that sets no flag can read as a brownout and hold the same way.
- **Brownouts are not a reset storm.** Brownouts keep the backup registers, so without a rule KM0 would count a brownout reset like any other unrequested one. A pack weak enough to brown out at idle (Wi-Fi transmit peaks) could then reach the storm limit of five within ten minutes, and a confirmed bank 1 would give way to bank 0, an older build, for a supply fault rather than a build fault. So KM0 reads `BOOT_Reason` before its trial step, and on a brownout boot takes back the count the previous boot left pending (`trial::decide_after`, `PLTRL1 brownout=1 reg=…`): the brownout neither counts nor ends a burst. The bit may be stale: the loader that backs the ROM's flags up into `BOOT_Reason` returns early when it finds none set, so after one real brownout a reset that sets no flag can read as a brownout too. So no two boots in a row are forgiven (`trial::FORGIVEN`), and a fault still reaches the storm limit within 10 early resets however the bit reads. A trial still counts a brownout boot, so no trial is shortened or skipped ([OTA design](ota-design.md#reset-storms); `wellfed-hal-common/tests/trial_brownout.rs`). The floor makes a motor-driven brownout unlikely; it cannot prevent an idle one on a pack below the regulator's dropout.

## Sleep

Deferred; both cores keep their wakelocks. Static analysis:
- **Both cores hold `PMU_OS`** from `main` (`firstboot_km0.c`, `firstboot_km4.c`), and the feeding owner takes its own PMU wakelock when it opens. KM4's tickless idle only stops the tick while every task sleeps; with the wakelock held it never enters a low-power mode.
- **KM0's DLPS path is unmediated.** On entering sleep, the SDK's tickless code reaches flash users (the flash lock and its clock changes) outside the FW06 flash authority, so a sleep entry could touch flash while KM4 holds a lease. That carried item stays open: before KM0 may sleep, its flash users must go through the authority or a KM0 deny provider must refuse them.
- **The outputs, the watchdog and the clock.** Sleeping safely needs the motor verified at STOP and the emitters off before entry, the runtime's watchdog either stopped or fed across the sleep, the 32 kHz clocks the owner's timing and the clock (FW46) rely on kept, and a wake that resumes no feed (the journal already ensures none is resumed across a reset; a sleep is not a reset). None of that is built.
- **Where the saving is.** On batteries, stock shuts Wi-Fi and MQTT 5 s after the adapter leaves and duty-cycles its beams (static, FP14); the stock inventory does not record whether it enters the SoC's low-power modes. Dropping Wi-Fi on the pack is the larger saving and is battery-mode policy (FP14), which needs no SoC sleep.

## Tests

- **Core:** `wellfed-core/tests/supply.rs` (readings, the floor's hold, cutoff, hysteresis and wrap) and `tests/sensor_inputs.rs` (a surge under the floor tolerated, a long sag or one under the cutoff not, the clear level, the adapter's verdict, a failed conversion).
- **HAL:** `wellfed-hal/tests/power.rs` (unread, stale and implausible readings from the monitor's own samples); `wellfed-hal-common/tests/trial_brownout.rs` (a brownout bit stuck set still gives way within 10 resets and holds, then halts, on bank 0; never two forgiven in a row; a trial still counts a brownout boot; a brownout in a held, halted or spent-trial boot, and after a settled or deliberate one).
- **Product:** `wellfed-feeder/tests/supply.rs` (events and their rate limit, the brownout hold, readiness order); the firmware's `tests/manual_feed.rs` (a press refused on a low pack, the adapter's loss as a `Supply` event from the board's samples); `wellfed-client/tests/api.rs` (the battery reason to a `POWER` session and as `Starting` to an older one); wire fixtures appended for every new variant.
- **Simulation** (the real owner on the fake SDK, `wellfed-sim`; the plant now sags the battery channel by `surge_sag_adc` and `drive_sag_adc` while it drives on the pack, both unmeasured and zero by default): the adapter pulled mid-feed fails it and nothing drives until a new feed, which completes on the pack; a surge under the floor is tolerated; a sustained sag or one under the cutoff fails the feed; under the floor and in the band no feed drives, at the clear level one does. Checked-in scenarios: `adapter-loss-mid-feed.json`, `battery-sag.json`.

## On the bench (for the owner)

Physical acceptance needs the cells in, which the run's rules leave to the owner. The order below keeps motion out of the first steps.
1. **Readings at rest.** Fit three fresh D cells, adapter in. `wellfed stats --section power` and a subscriber's `Supply` sample should read `Reading` near the meter's voltage, `below_floor: false`. Pull the adapter with relay 1: one `Supply` event (`adapter: false`), readiness `Ready`. Restore: one more after a minute at most.
2. **The floor without motion.** Replace the cells with a bench supply at the pack terminals and take the adapter out, starting at 4.5 V. Step it down: readiness turns `Battery` within a poll of the pin reading under 1650 for 300 ms (about 3.40 V), and back to `Ready` only at about 3.53 V. Note the voltage at which the board resets, if it does, and whether the next boot's reset cause is `Brownout`: that is the margin the floor is provisional on.
3. **Under load** (needs an authorization covering motion with the cells or supply in): a one-portion feed on fresh cells, with the analyzer on the motor and the supply's voltage logged, measures the real surge and drive sag; then the same at a supply voltage just above the floor. Those figures replace the plant's zero sags and confirm or move `FLOOR`.
4. **Brownout hold.** After a brownout reset on the supply, readiness stays `Battery` until the adapter returns, . **The brownout bit is fresh.** After that brownout, with the adapter in, force a watchdog reset (`wellfed reboot` is deliberate and does not count; use a build or probe that stalls the watchdog, such as the `owner-stall` variant). The next boot must not log `PLTRL1 brownout=1`, and its `Boot` event's reset flags must not carry bit 18. If it does, the bit is stale on this board: the bound above still holds, but record it.

## Open

- The floor's figures under load and the board's brownout level (above).
- Whether `BOOT_Reason`'s brownout bit is set by a real brownout here (the storm breaker's brownout rule and the brownout hold both rest on it).
- Sleep, and with it the KM0 flash users on the DLPS path.
- Battery-mode behaviour and a battery percentage on stock's curve (FP14).

# Power

The feeder runs from its adapter, with three D cells as a backup. This page covers how the firmware reads the supply and the pack's charge, what the feeder does on its batteries, the battery floor below which it will not feed, what happens after a brownout, and why the chip does not sleep yet.

Status: **Experimental**. The inputs are observed on our unit. Battery mode, the charge, the floor, the brownout handling and the supply reporting are host-tested and simulated. None has run on the pack under load, and battery mode has not run on hardware.

## Inputs

- **Pack.** ADC channel 5, through a divider of about 2.063 (`power::DIVIDER_PER_MILLE`). Fresh cells read 2357 mV at the pin with the adapter in (4.85 V on a meter) and 2306 mV without (4.77 V). With no pack the pin reads about 16 mV.
- **Adapter.** PA19, HIGH while the adapter is in. The feeder rides through both transitions without a reset.
- **Motor current.** ADC channel 4 ([jams and sensor faults](jams-and-sensor-faults.md)).
- **Chip supply.** Internal ADC channel 10, in millivolts with stock's scaling ([bring-up probes](bring-up-probes.md#power-input-probe)).

The runtime's power monitor samples every 5 s. In the `feeder` firmware the feeding owner holds the ADC and samples on each of its 20 ms polls instead. Both publish the same sample (`wellfed_hal::power`).

**Charge.** A plausible pack reading carries a percentage on stock's curve, in the channel's scaled unit. 1650 mV at the pin (the floor, a pack of about 3.40 V) is 0 %, and each 60 mV above it is a 10 % step, up to 2250 mV (about 4.64 V) at 100 %. A reading reports the first step at or above it, so 1651 to 1710 read 10 %, and anything from 2191 up reads 100 %. Fresh cells read about 2306 at rest: 100 %. The curve is `power::PACK.curve` (`wellfed_core::supply::Curve`).

The monitor logs `PLBAT1` records, in every firmware that does not claim the ADC: `start created=<0|1>`, `open ok=<0|1>` once the startup probe has released the ADC, `sample pin_mv=<n> adapter=<0|1>` for the first sample and each adapter change, `error adc` if a conversion fails (the monitor then stops), or `claimed` when the firmware keeps the ADC.

## The supply on the wire

`Capabilities::POWER` (bit 128) carries the supply as the feeder judges it. An unknown is never passed off as a value.

- **`Supply { adapter, battery, below_floor }`.** `adapter` is `None` without a fresh sample. `battery` is one of:
  - `Absent`, no pack;
  - `Reading { mv, percent }`, the pack's millivolts and its charge;
  - `Implausible { mv }`, above 5.5 V, or below 2.5 V on the pack alone, where the board could not be running;
  - `Unread`, no sample: the monitor has not run, its ADC failed, or the firmware does not sample;
  - `Stale { age_ms }`, older than three monitor periods.

  `below_floor` is the feeding owner's verdict on the pack's floor, `None` before it has judged and in a firmware without an owner. The judging is `wellfed_core::supply::classify`, against the board's `power::PACK`.
- **Status.** A session that claims `POWER` gets the supply in its status (`Outcome::FeederStatus { supply }`), and `wellfed status` prints it.
- **`Metric::Supply`** goes to subscribers with the bit at each power sample (5 s), beside the version 1 `Power` metric, which is unchanged.
- **`EventBody::Supply`** records the adapter coming or going and the pack crossing its floor, at most one a minute (`feeder::supply::EVENT_MS`). Changes inside the minute are recorded once it ends, as the supply then stands, so a chattering adapter or a pack hovering at its floor costs one event a minute. A boot's first reading is a baseline, not an event, and a stale sample changes nothing.
- **Readiness** gains `Blocked::Battery` ([below](#the-floor)), refused `Busy`. A session that claims `READINESS` but not `POWER` sees it as `Blocked::Starting` (`Blocked::seen_by`), the reason that also refuses `Busy` and clears by itself. It is not lasting, so it is never a `FeedingBlocked` event; the `Supply` event records it instead.
- **Statistics.** The `Power` section's battery history, least and most take only `Reading` values, so an implausible reading never enters them.

Nothing is stored in a new place: the events go through the existing ring in its existing 16-byte records. Bank 0 runs an older feeder that cannot decode a `Supply` body. It skips such records as a one-event gap, as it does any newer body, so no bank-0 refresh is needed.

## Battery mode

On its batteries the feeder does what stock does ([stock firmware](../devices/plaf103/stock/README.md#power-and-battery)):

| | On batteries |
| --- | --- |
| Wi-Fi | Off 5 s after the adapter goes, back on the moment it returns. No app, CLI or server reaches the feeder meanwhile. |
| Meals | Run from the running clock. |
| Buttons, lock and lights | Work as on the adapter. |
| Updates | An offer is refused `Update(Battery)` as soon as the adapter reads absent. |
| Below the floor | No feed starts ([below](#the-floor)). |

**When it starts.** The loop enters battery mode once every fresh sample for 5 s (`feeder::supply::RADIO_OFF_MS`) has read the adapter absent with a plausible pack. A shorter loss changes nothing. Only a fresh sample moves the mode: an unknown adapter, or a pack that reads absent or implausible on the pack alone, neither starts the wait nor ends it.

**The phantom at a power cut.** With no cells fitted, cutting the adapter reads the decaying 5 V rail as a plausible pack (3.86 V was seen) for the moment before the board stops. That moment is far shorter than the 5 s wait, so it never turns Wi-Fi off. It cannot lift a brownout hold either, which needs 3 minutes ([below](#brownout)). It does leave one `Supply` event with that reading, which is what the rail read.

**What "Wi-Fi off" does.** The loop asks the platform for the radio off on every pass (`Platform::radio`). On the feeder, the C Wi-Fi task then:

1. leaves the network as it does before a restart: it stops DHCP, releases the address, disconnects, and waits up to 1 s for the association to go, so the access point drops it rather than holding it against the next join;
2. calls the vendor's `wifi_off`, which takes the interface down and turns the WLAN driver and radio off;
3. waits, polling every 50 ms, until the radio is asked on again.

When the adapter returns it calls `wifi_on` in station mode, with the same power-save settings as at boot, and joins afresh with the rejoin backoff reset. A `wifi_on` that fails is tried again every 5 s. A restart asked for meanwhile parks the task with the radio off. A join cut short this way does not count toward the stored credentials' fallback. The UART shows `PLBAT2 radio off|on` from the loop, and `PLWIF3 … kind=off`, then `kind=on`, from the task.

**Setup mode keeps the radio on.** The vendor's BLE stack runs only while the WLAN driver is up, so BLE setup mode holds Wi-Fi on even on batteries. Setup mode already stops advertising without the adapter ([Bluetooth](bluetooth.md)). Once setup mode ends (with a restart), battery mode turns Wi-Fi off again.

**The clock.** Turning Wi-Fi off does not touch the [clock](clock.md). It stays valid until its error bound runs out, as it does with no time source: about 6.6 hours after the last sync while its rate is unlearned, about 22 hours once learned. Meals then hold until the adapter returns and a source syncs it. NTP simply goes unanswered while the radio is off.

## The floor

The pack's floor is `wellfed_hal::power::FLOOR`, in the battery channel's scaled unit (pin millivolts). It is **provisional**: no motor has run on the pack yet, and the regulator's dropout and the chip's brownout level are unmeasured on this board. The SDK leaves the brownout thresholds at their reset values and documents them only as codes.

| Limit | Pin mV | Pack | Why |
| --- | ---: | ---: | --- |
| `floor` | 1650 | 3.40 V (1.13 V a cell) | Stock's 0 % point, below which stock stops its outputs on battery ([stock firmware](../devices/plaf103/stock/README.md)) |
| `clear` | 1710 | 3.53 V | Stock's 10 % step: once below, a pack must recover by a step before feeding again |
| `cutoff` | 1550 | 3.20 V | A sag this deep leaves the 3.3 V regulator little headroom, so stop at once rather than risk a brownout mid-drive |
| `hold_ms` | 300 | | Fifteen 20 ms polls. The start-up surge lasted one poll (on the adapter); a sag that outlasts the hold is the pack failing under load |

`wellfed_core::supply::Floor` applies them on every acquisition the owner takes, on the adapter too, so the verdict is current when the adapter is pulled:

- a reading at or above `clear` is above the floor;
- between `floor` and `clear` a pack keeps its verdict, and a first reading there counts as above;
- under `floor` for `hold_ms` in a row (a failed conversion counts as under), or under `cutoff` at all, is below.

Where it acts:

- **A running feed on the pack alone** (PA19 LOW) stops when the pack is no longer judged above the floor, or its conversion fails. The controller's power fault ends it `Failed`, `Power`, with STOP applied and the result written as any failure. It is never restarted; a client or schedule must ask again.
- **A new feed on the pack** is refused before it reaches the owner. `Feeding::blocked` answers `Battery` while `OwnerStatus::supply_low` holds, and readiness reports it. The controller's own start check stays behind it: a feed that lost the race ends `Failed`, `Power` without driving, once its prepare window runs out.
- **A meal due on a low pack** waits and is retried within its 15-minute grace, then recorded `Missed { reason: Blocked(Battery) }` ([schedules](schedules.md)). It is never fed late.
- **On the adapter** the floor never refuses or stops a feed.
- **An adapter change during a feed stops it** (`Failed`, `Power`). The change unsettles power for 100 ms (stock's settle), and a feed does not ride across a change of source. The next feed runs on whichever supply is there.

A pack that sags under load and recovers above `clear` at rest will feed again, and may fail again. Each feed ends with an explicit result and none is retried, so this costs failed feeds, not motion without a result.

**Low battery, in short.** On the pack alone below the floor, the feeder refuses every feed with `Blocked::Battery` (`Busy`), records missed meals as blocked by it, fails a running feed with `Power`, and blinks the warning light. Otherwise it keeps running: the buttons, the lock and the clock work, and it feeds again once the pack reads `clear` or the adapter returns. Stock instead suspends below 0 %: it stops its motor, emitters and lights and polls for mains every second.

## Brownout

A brownout is a reset caused by the supply dropping too low. Three rules keep it from feeding twice, looping, or rolling back a good build:

- **No feed is replayed.** A feed a brownout interrupted is resolved as `Uncertain` by the next boot's journal recovery, under its own id, and never resumed.
- **The next feed waits for the adapter or a recovered pack.** A boot whose cause is brownout holds feeding as `Blocked::Battery` (`feeder::supply::Watch`, logged `PLBAT2 hold brownout`). At rest the pack recovers, so its reading at once would let the next feed start and sag it into the same brownout: a loop of resets, each leaving a feed uncertain. The hold ends when the adapter is seen. It also ends once every fresh reading for 3 minutes (`RELEASE_MS`) has been 50 % or more (`RELEASE_PERCENT`): a pin reading above 1890 mV, the floor plus 240 mV, or about 3.9 V at the pack. A reading under it, or any sample that is not a fresh plausible reading, starts the 3 minutes again. Fresh cells read 100 %, so a battery-only feeder feeds again on new cells. A pack that browned out and rebounds at rest stays well under 50 %. Cells fitted after a power cycle start a power-on boot, which never holds. The release is logged `PLBAT2 hold released by=adapter|pack`.
- **Brownouts are not a reset storm.** Brownouts keep the backup registers, so without a rule KM0 would count a brownout reset like any other unrequested one. A pack weak enough to brown out at idle, on Wi-Fi transmit peaks, could then reach the storm limit of five within ten minutes. A confirmed bank 1 would then give way to bank 0, an older build, for a supply fault rather than a build fault. So KM0 reads `BOOT_Reason` before its trial step, and on a brownout boot takes back the count the previous boot left pending (`trial::decide_after`, `PLTRL1 brownout=1 reg=…`). The brownout neither counts nor ends a burst. A trial still counts a brownout boot, so no trial is shortened or skipped ([OTA design](ota-design.md)).

The cause comes from the ROM's flags: `BOOT_Reason` bit 18 is brownout. The statistics report it (`ResetCause::Brownout`), and the `Boot` event carries the raw flags.

The bit may be stale. The loader that copies the ROM's flags into `BOOT_Reason` returns early when it finds none set, so after one real brownout, a reset that sets no flag can read as a brownout too. Two guards follow:

- No two boots in a row are forgiven (`trial::FORGIVEN`; bit 31 of backup register 4 marks a forgiven boot, and the register's identity tag is now 7 bits). A fault still gives way within 10 early resets, and halts within 16, however the bit reads.
- A later reset with no flag can read as a brownout and hold feeds the same way: until the adapter is seen, or for 3 minutes on a charged pack.

The floor makes a motor-driven brownout unlikely. It cannot prevent an idle one on a pack below the regulator's dropout.

## Sleep

Sleep is deferred, and both cores keep their wakelocks (recovered):

- **Both cores hold `PMU_OS`** from `main` (`firstboot_km0.c`, `firstboot_km4.c`), and the feeding owner takes its own PMU wakelock when it opens. KM4's tickless idle only stops the tick while every task sleeps; with the wakelock held it never enters a low-power mode.
- **KM0's DLPS path is unmediated.** On entering sleep, the SDK's tickless code reaches flash users (the flash lock and its clock changes) outside the flash authority, so a sleep entry could touch flash while KM4 holds a lease. Before KM0 may sleep, its flash users must go through the authority, or a KM0 deny provider must refuse them.
- **What safe sleep needs.** The motor verified at STOP and the emitters off before entry. The runtime's watchdog either stopped or fed across the sleep. The 32 kHz clocks that the owner's timing and the [clock](clock.md) rely on kept running. And a wake that resumes no feed: the journal already ensures none is resumed across a reset, but a sleep is not a reset. None of that is built.
- **Where the saving is.** On batteries, stock shuts Wi-Fi and MQTT 5 s after the adapter leaves and duty-cycles its beams (recovered). The stock inventory does not record whether it enters the chip's low-power modes. Dropping Wi-Fi on the pack is the larger saving, and [battery mode](#battery-mode) does it with no chip sleep. The beams are not duty-cycled yet.

## Bench checks

Physical acceptance needs the cells in, which is the owner's call. The order keeps motion out of the first steps.

1. **Readings at rest.** Fit three fresh D cells, adapter in. `wellfed stats --section power` and a subscriber's `Supply` sample should read `Reading` near the meter's voltage, with `below_floor: false`. Pull the adapter with relay 1: you should see one `Supply` event (`adapter: false`) and readiness `Ready`. Restore it: one more event within a minute.
2. **The floor without motion.** Replace the cells with a bench supply at the pack terminals, take the adapter out, and start at 4.5 V. Step it down. Readiness turns `Battery` within a poll of the pin reading under 1650 for 300 ms (about 3.40 V), and returns to `Ready` only at about 3.53 V. Note the voltage at which the board resets, if it does, and whether the next boot's reset cause is `Brownout`. That is the margin the floor is provisional on.
3. **Under load.** This needs an authorization covering motion with the cells or supply in. Run a one-portion feed on fresh cells, with the analyzer on the motor and the supply's voltage logged, to measure the real surge and drive sag. Then repeat at a supply voltage just above the floor. Those figures replace the simulator's zero sags and confirm or move `FLOOR`.
4. **Brownout hold.** After a brownout reset on the supply with the adapter out, readiness stays `Battery` and the UART shows `PLBAT2 hold brownout`. Raise the supply to 4.5 V: the hold ends 3 minutes later (`PLBAT2 hold released by=pack`). Repeat, and fit the adapter instead: it ends at once (`by=adapter`).
5. **Battery mode.** With fresh cells and the adapter in, run `wellfed events --follow` and keep a UART capture. Pull the adapter with relay 1. Within the first 5 s, `wellfed status` shows `adapter: false` with the charge, and an update offer is refused `battery`. After 5 s the UART shows `PLBAT2 radio off` and `PLWIF3 … kind=off`, the session drops, and the feeder leaves the access point's client list. Press Dispense on an unlocked panel: it feeds (hopper empty, one portion). Restore the adapter: you should see `PLBAT2 radio on`, `kind=on` and a join, and the CLI reconnects. The events show the `Supply` changes and the feed. Leave it on batteries across a scheduled meal to see it run from the running clock. Pull the adapter several times: each `wifi_off` and `wifi_on` must return, with no hang or reset.
6. **The brownout bit is fresh.** After the brownout of step 4, with the adapter in, force a watchdog reset with a build or probe that stalls the watchdog, such as the `owner-stall` variant. (`wellfed reboot` is deliberate and does not count.) The next boot must not log `PLTRL1 brownout=1`, and its `Boot` event's reset flags must not carry bit 18. If it does, the bit is stale on this board: the bounds above still hold, but record it.

## Open questions

- The floor's figures under load, and the board's brownout level.
- Whether a real brownout sets `BOOT_Reason`'s bit here. The storm breaker's brownout rule and the brownout hold both rest on it.
- Sleep, and with it the KM0 flash users on the DLPS path.
- Whether the vendor's `wifi_off` and `wifi_on` cycle cleanly on this board, again and again, and how much current battery mode saves.
- Stock lights its warning at 20 % or less whenever a pack reads, and duty-cycles its beams on batteries. Neither is built.

## Evidence

- **Pack and adapter inputs.** Observed (October 23, UART and meter, the `adc` probe, three D cells, no reboot between states):

  | State | PA19 | Channel 5 mean (raw) | Scaled (mV) | Pack on meter |
  | --- | --- | --- | --- | --- |
  | Adapter, no batteries | 1 | 240 | 16 | — |
  | Adapter and batteries | 1 | 2849 | 2357 | 4.85 V |
  | Batteries only | 0 | 2737 just after the unplug, settling to about 2793 over 20 s | 2256 to about 2306 | 4.77 V |
  | Adapter back in | 1 | 2848 | 2356 | — |

  So channel 5 measures the pack, not the adapter, through a divider of about 2.06 (2.058 and 2.068 in the two states). Channel 4 stayed at 240 throughout, and the feeder neither reset nor showed anything visible at either transition. Channel 5 is noisier than the others: about 110 raw between the minimum and maximum of each 16-conversion sample, against 3 or 4 on channels 4 and 10. The meter readings were not time-stamped against particular samples.
- **Health with a pack.** Observed (October 23): health read `"adapter":true,"battery_mv":4862` with the adapter (meter earlier: 4.85 V), and `"adapter":false,"battery_mv":4755` with it pulled (meter: 4.77 V). With no pack, the monitor logged `PLBAT1 open ok=1` only after the startup probe ended, then `sample pin_mv=15 adapter=1`.
- **Surge.** Observed: the start-up surge lasted one 20 ms poll on the adapter.
- **Power cut mid-drive.** Observed: an interrupted feed resolves as uncertain at the next boot and is never resumed ([motor and feeds](motor-pulse.md#a-power-cut-mid-feed)).
- **Stock's floor and battery behavior.** Recovered from the stock firmware.
- **The floor, supply reporting and brownout rules.** Modeled (October 26):
  - Core: `wellfed-core/tests/supply.rs` (readings, the floor's hold, cutoff, hysteresis and wrap) and `tests/sensor_inputs.rs` (a surge under the floor tolerated, a long sag or one under the cutoff not, the clear level, the adapter's verdict, a failed conversion).
  - HAL: `wellfed-hal/tests/power.rs` (unread, stale and implausible readings from the monitor's own samples) and `wellfed-hal-common/tests/trial_brownout.rs` (a brownout bit stuck set still gives way within 10 resets and holds, then halts, on bank 0; never two forgiven in a row; a trial still counts a brownout boot; a brownout in a held, halted or spent-trial boot, and after a settled or deliberate one).
  - Product: `wellfed-feeder/tests/supply.rs` (events and their rate limit, the brownout hold, readiness order); the firmware's `tests/manual_feed.rs` (a press refused on a low pack, the adapter's loss as a `Supply` event from the board's samples); `wellfed-client/tests/api.rs` (the battery reason to a `POWER` session, and as `Starting` to an older one); wire fixtures for every new variant.
  - Simulation: the real owner on the fake SDK (`wellfed-sim`). The plant sags the battery channel by `surge_sag_adc` and `drive_sag_adc` while it drives on the pack, both unmeasured and zero by default. The adapter pulled mid-feed fails it, and nothing drives until a new feed, which completes on the pack. A surge under the floor is tolerated, and a sustained sag or one under the cutoff fails the feed. Under the floor and in the band no feed drives; at the clear level one does. Checked-in scenarios: `adapter-loss-mid-feed.json`, `battery-sag.json`.
- **The brownout bit on hardware.** Unknown: whether a real brownout on this board sets bit 18.
- **The phantom at a cut.** Observed (October 28, the server's bench run): a relay-1 cut with no cells recorded `Supply { adapter: false, battery: Reading 3861 mV }`, the decaying rail.
- **Battery mode, the charge and the hold's release.** Modeled (October 28):
  - Core: `wellfed-core/tests/supply.rs` (stock's steps at every boundary, a reading's charge).
  - Product: `wellfed-feeder/tests/supply.rs` (the 5 s wait, the radio back at once, a short loss, a boot on batteries across the uptime's wrap, unknown or implausible packs that neither start nor end the wait, setup mode, the rail at a cut, the hold lifted by the adapter or by 3 minutes at 50 %, and every interruption restarting it).
  - HAL and runtime: `wellfed-hal/tests/link.rs` (the radio's leave, and a restart winning over it) and `wellfed-feeder-runtime/tests/network.rs` (joins cut short never count toward the fallback).
  - Simulation: `wellfed-feeder-sim/tests/battery.rs`. In the loop: the radio and the address gone, breakfast fed on batteries, a meal missed as blocked on a flat pack, the hold lifted on fresh cells, the rail at a cut. Over TCP: the status's supply and charge, an offer refused `Battery`, the session lost and back. The simulator's console takes `adapter in|out` and `pack <mV>|out`.
  - The C task's `wifi_off` and `wifi_on` path is static analysis only.

# Feeding schedules

The feeder keeps up to ten feeding plans and runs them itself, with no server. This page explains what a plan is, when one runs, and how the feeder makes sure a meal runs at most once through restarts, clock jumps and daylight changes. It is for owners who want to know what to expect and for anyone changing the schedule code.

Status: **Works**. Observed on our feeder; daylight changes and power cuts between a claim and its feed are tested in simulation only. The meal reminder is **Experimental**: tested in simulation only.

## Plans

A plan feeds a number of portions at a time of the feeder's local day, on the weekdays it lists. A plan that lists no weekday runs once, at its next occurrence, and then disables itself; it stays in its slot so you can enable it again.

| Field | Limits |
| --- | --- |
| Time | 00:00 to 23:59, local time |
| Portions | 1 to 48, as a manual feed |
| Days | Any of Monday to Sunday, or none for once |
| Enabled | On or off |

The feeder has ten slots, as stock keeps at most ten plans. The slot number is the plan's name in the history and in `wellfed plans`.

```sh
wellfed plans list
wellfed plans add --at 07:30 --portions 2 --days daily
wellfed plans edit 0 --days mon,wed,fri
wellfed plans disable 0
wellfed plans delete 0
```

Owners and members may change the plans, as they may change the settings. Each change reads the plans, edits that table and writes it back against the generation it read. If another client changed the plans meanwhile, or a one-off plan disabled itself, the write is refused `Stale` and nothing changes; read them again and retry. The command never retries by itself.

A plan you add, enable again, or whose time or days you change, runs only from its next occurrence after the save. Saving a plan at 07:35 for 07:30 does not feed until tomorrow. Changing only a plan's portions keeps its occurrences as they were.

## When a plan runs

The feeder needs a valid clock with a zone offset to run plans. Until a time source sets the clock after a cold boot, plans hold and manual feeds still work ([clock](clock.md)).

Each occurrence goes through the missed-meal rule ([clock](clock.md#missed-meal-rule)):

- **On time.** It runs at its minute, through the same feeding owner, readiness and journal as a Dispense press.
- **Late, within 15 minutes.** After a restart, a forward clock jump, or a wait for the feeder to be ready, it still runs once.
- **Later than 15 minutes.** It never runs. The history records it as `Missed`, with the reason.
- **Already run.** A backward clock jump or a restart never runs it again.

A plan due while the feeder cannot feed waits: another feed, a Wi-Fi join, an install, BLE setup or a battery hold. On its batteries the feeder keeps running plans from its running clock with Wi-Fi off ([battery mode](power.md#battery-mode)). It runs as soon as the feeder is ready, within the 15 minutes. Plans due at the same minute run one after another, by slot.

A feed a plan started is a `Fed` event by `schedule` with the plan's slot. A missed occurrence is a `Missed` event:

| Reason | Meaning |
| --- | --- |
| `late` | The feeder was off or had no time, or the clock moved past it |
| `blocked` | It waited past 15 minutes; the event names what blocked it |
| `interrupted` | The feeder stopped after taking the occurrence and before the feed was reserved |

A plan you saved while the feeder had no time has no starting point, so it starts once the clock returns. An occurrence that falls before then, late or not, passes without a `Missed` event: it may have fallen before the save.

## Meal reminder

The feeder can announce each scheduled meal ahead of it, so the app, the server or a home automation can tell you food is coming. It records a `MealSoon` event on the event stream; it plays no sound and sends no phone notification itself. The reminder is off until you set it:

```sh
wellfed settings --meal-reminder 10    # 10 minutes before each meal
wellfed settings --meal-reminder off
```

The lead is 1 to 60 minutes, one setting for every plan. Stock kept a similar field in its cloud (`feedingPlanAdvanceNoticeTime`, 5 or unset), with no unit we could find, so the range is ours.

A `MealSoon` event names the plan's slot, the time the meal is due (and its minute of the feeder's local day) and the portions it will ask for. The feeder sends one for each occurrence:

- **When the lead starts.** With a 10-minute lead, a 07:30 meal is announced at 07:20. A plan you save, or move, inside the lead is announced at once.
- **Only for a meal that will run.** A disabled plan, a feeder without a valid clock, or a plans store it cannot write gets no reminder.
- **Once.** Changing only a plan's portions keeps the reminder already sent; the meal feeds the new portions. A change of time or days is a new occurrence, with its own reminder.
- **Across a daylight change**, the lead counts real minutes to the meal's instant: a 03:10 meal on the spring change day, 30 minutes ahead, is announced at 01:40, just before the clock jumps.

The reminder only reads the plans. It never claims, delays or blocks a meal, and the meal runs exactly as it would with the reminder off. It keeps nothing on flash about what it sent. So a restart inside the lead announces the same meal again once the clock is back, with the same plan and due time, and a client can drop the repeat.

## Daylight saving and time zones

Plans keep local time across daylight changes, from the zone's rule or a client's sync ([clock](clock.md#time-settings)):

- **A skipped time runs as far past the change as it was meant to be.** In spring, 02:30 runs at 03:30 when the clock jumps from 02:00 to 03:00.
- **A repeated time runs on its first pass.** In autumn, 01:30 runs once, in daylight time.

This is the rule RFC 5545 and JavaScript's Temporal call "compatible". Half-hour and quarter-hour zones work; stock rounded its zone to whole hours. The feeder works out each plan's occurrence once a minute, so a new zone applies to plans within a minute.

## At most once

The feeder never runs an occurrence twice, even if power fails at any moment. Each plan keeps a mark: the latest occurrence already run, missed or before the plan was saved. Nothing at or before the mark runs.

Running an occurrence takes two steps:

1. **The claim.** The writer saves the occurrence, the plan's slot and the feed journal's newest number. The claimed occurrence counts as the plan's mark from then on.
2. **The feed.** Only once the claim is on flash does the loop hand the feed to the feeding owner. Every other feed waits meanwhile, so the claimed feed is the very next number the journal reserves.

A cut between the two leaves a claim whose feed was never reserved. The next boot finds the journal where the claim left it, records the occurrence `interrupted`, and does not feed. A cut during the feed is the journal's usual case: the next boot resolves it `uncertain` and never runs it again. The event names the plan, since the claim says the feed was the plan's.

If the feeding owner refuses the claimed feed (a join began in between, say), the loop gives the occurrence back and claims it again 5 seconds later, until the 15 minutes run out.

A plans store the feeder cannot write runs nothing, since it could not keep the marks.

## How it differs from stock

| | Stock | Wellfed |
| --- | --- | --- |
| A plan due during another feed | Waits, with no limit, even across a restart | Waits up to 15 minutes, then is missed |
| A meal missed while off | Not caught up, not reported | Run if at most 15 minutes late, otherwise reported |
| Field checks | None ("99:99" is stored) | Every field checked |
| Time zone | Whole hours | Any offset, with daylight rules |
| A one-off plan after it runs | Deleted | Kept, disabled |
| A sound before the meal | Per plan | Not yet |
| A reminder before the meal | The vendor's cloud and app; nothing in the firmware | A `MealSoon` event on the feeder's event stream, 1 to 60 minutes ahead |

## Storage

The plans live in storage sectors 11 and 12 (`wellfed_feeder::store::plans`): one 128-byte record holding the ten plans, each plan's mark and the last claim. Only the settings writer writes it, inside a marked attempt, after a feed has released the outputs. A save happens for each change you make and for each occurrence run or missed. A record of a newer format makes the store read-only with no plans, so this build runs nothing it cannot read.

## Evidence

- **Observed, October 28, 2026** (`fp03-ota-20261028`, hopper empty): a daily plan fed once at its minute; a reboot within that minute, with the clock carried across it, ran nothing again; a plan set to run once fed and disabled itself; a plan due while the feeder had no clock after a cold boot held, and the sync 16 minutes later recorded it missed, unfed.
- **Simulated, October 28, 2026.** The meal reminder, on the same loop: off by default, its lead before the meal with the meal still run once, a disabled plan and a feeder without time, a plan edited inside the lead, a restart inside the lead, and Denver's spring and autumn changes.
- **Simulated, October 28, 2026.** `wellfed-feeder-sim`'s schedule tests run the product loop in virtual time over the real stores and feed journal: on-time feeds, restarts, backward and forward jumps, a cold boot without time, plans due together, a wait past the grace, a cut between the claim and the feed, a cut mid-feed, a refused feed and its retries, one-off plans enabled again, a plan saved without time, and Denver's spring and autumn changes.

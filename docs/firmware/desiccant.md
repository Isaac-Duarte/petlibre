# Desiccant reminder

The hopper's desiccant pack keeps the food dry, and it needs replacing every few weeks. The feeder counts the days since you last replaced it and reports when it is due, with no server and no cloud. This page explains how the count works, what you see while the clock is unknown, and where it is stored. It is for owners and for anyone changing the reminder.

Status: **Experimental**. Built and tested on the host and in simulation; not yet run on the feeder.

## Using it

After you put in a fresh pack, record it:

```sh
wellfed desiccant --reset
```

Then read where the count stands at any time:

```sh
wellfed desiccant
```

The period between replacements is a setting, 1 to 90 days, 14 until you change it:

```sh
wellfed settings --desiccant-days 21
```

Through the server, the same commands are `wellfed server desiccant [--reset]` and `wellfed server settings --desiccant-days`.

Owners and members may record a replacement, as they may change the plans. So may the server, and the period is a setting like any other: a change names the generation it read.

## The count

The feeder keeps the period and the date of the last replacement in its settings, never a running count. So the reminder survives any restart and costs no flash write as days pass.

Days are calendar days of the feeder's local time. A replacement on day D with a period of P days leaves P days that day, and P − n days n days later. It is due on day D + P, when 0 days are left, and the count keeps going below zero after that, as stock's cloud reported it. A daylight change makes one day 23 or 25 hours long, but the count still moves by one at each local midnight.

The status reports one of:

| State | Meaning |
| --- | --- |
| `unset` | No replacement recorded yet. Record one once the pack is fresh. |
| `unknown` | The clock is invalid, so today's date is unknown. |
| `remaining` | `days` left of the period, and `due` from 0 on. |

The feeder has no battery-backed clock. After a cold boot the count is `unknown` until a time source sets the clock ([clock](clock.md)). With no zone and no offset from a sync, local time is UTC.

## Recording a replacement

A replacement is dated by the feeder's own clock, never by the client's. While the clock is invalid, the feeder refuses the reset with `NoClock` and records nothing: sync the time (`wellfed time`) and try again. A guessed date would make the count wrong for weeks.

A reset is answered `Done` with the settings' new generation, and records a `Settings` event. If the clock was moved back after a reset, so the recorded day is later than today, the count shows the whole period until that day passes.

## How it differs from stock

| | Stock | Wellfed |
| --- | --- | --- |
| Where it lives | The vendor's cloud | The feeder |
| Works with the app alone | No | Yes |
| Period | 14 days observed; range unknown | 1 to 90 days, 14 by default |
| Below zero | Keeps counting | Keeps counting |
| Notification | Phone push from the cloud | Due state in the status; server alerts are not built |

## Storage

The period and the replacement day are fields of the settings record, version 3, in storage sectors 2 and 3 ([device API](device-api.md#settings-wi-fi-and-the-lock)):

| Bytes | Field |
| --- | --- |
| 44 | The period in days, 1–90 |
| 45 | 1 when a replacement is recorded, else 0 |
| 46–47 | Zero |
| 48–51 | The replacement's local day, days since 1970-01-01 (i32), zero when none |

A record of an older version reads as erased: the settings open on their defaults and the next save replaces it.

## Evidence

- **Modeled, October 28, 2026.** Host tests cover the count through zero and below, a reset dated after today, the local day with and without an offset, and Denver's spring and autumn changes (`wellfed-core`'s `desiccant` tests). The settings store's tests save and reopen the fields, refuse a period outside its limits, treat a bad field as corrupt and keep the old or new record through a power cut at every byte. Over the device API, `wellfed-client`'s tests, the conformance suite on both platforms, `wellfed-feeder-sim` across a restart and the server through its journal all record a replacement, refuse one without a clock and report the count.

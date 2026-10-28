//! When a plan's occurrences fall: local minutes to UTC across daylight
//! changes, weekday masks, plans that run once, and the zones stock could
//! not keep.

use proptest::prelude::*;
use wellfed_core::clock::zone::{Rule, Zone};
use wellfed_core::schedule::{
    PLANS, Plan, Weekdays, latest_due, next_due, table_valid, utc_of, weekday,
};

const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;

fn rule(text: &str) -> Rule {
    Zone::parse(text.as_bytes()).unwrap().rule()
}

fn offsets(rule: Rule) -> impl Fn(u64) -> Option<i32> {
    move |ms| Some(rule.offset_at(i64::try_from(ms / 1000).unwrap()))
}

fn daily(minute: u16) -> Plan {
    Plan {
        minute,
        portions: 2,
        days: Weekdays::EVERY_DAY,
        enabled: true,
        call: 0,
    }
}

// 2026-03-08T09:00:00Z: Denver goes from 02:00 MST to 03:00 MDT.
const SPRING: u64 = 1_772_960_400_000;
// 2026-11-01T08:00:00Z: Denver goes from 02:00 MDT back to 01:00 MST.
const FALL: u64 = 1_793_520_000_000;
// 2026-10-28T00:00:00Z, a Wednesday.
const WEDNESDAY: u64 = 1_793_145_600_000;

#[test]
fn an_occurrence_is_the_local_minute_in_utc() {
    let denver = offsets(rule("MST7MDT,M3.2.0,M11.1.0"));
    // 07:30 MDT on Wednesday is 13:30Z.
    let due = WEDNESDAY + 13 * HOUR_MS + 30 * MINUTE_MS;

    assert_eq!(latest_due(&daily(7 * 60 + 30), due, &denver), Some(due));
    assert_eq!(latest_due(&daily(7 * 60 + 30), due + 1, &denver), Some(due));
    // A millisecond before, the latest is the day before's.
    let yesterday = due - 24 * HOUR_MS;
    assert_eq!(
        latest_due(&daily(7 * 60 + 30), due - 1, &denver),
        Some(yesterday)
    );
}

#[test]
fn a_minute_the_spring_change_skips_runs_as_far_past_the_change() {
    let denver = offsets(rule("MST7MDT,M3.2.0,M11.1.0"));
    // 02:30 does not exist on March 8: it runs at 03:30 MDT, 09:30Z.
    let local = i64::try_from(SPRING).unwrap() - 7 * 3_600_000 + 30 * 60_000;

    assert_eq!(utc_of(local, &denver), Some(SPRING + 30 * MINUTE_MS));
    assert_eq!(
        latest_due(&daily(2 * 60 + 30), SPRING + HOUR_MS, &denver),
        Some(SPRING + 30 * MINUTE_MS)
    );
    // 01:59 MST still exists, a minute before the change.
    assert_eq!(
        latest_due(&daily(60 + 59), SPRING, &denver),
        Some(SPRING - MINUTE_MS)
    );
}

#[test]
fn a_minute_the_fall_change_repeats_runs_on_its_first_pass() {
    let denver = offsets(rule("MST7MDT,M3.2.0,M11.1.0"));
    // 01:30 happens at 07:30Z (MDT) and again at 08:30Z (MST).
    let first = FALL - 30 * MINUTE_MS;
    let second = FALL + 30 * MINUTE_MS;

    assert_eq!(latest_due(&daily(90), first, &denver), Some(first));
    // At the second pass the latest occurrence is still the first.
    assert_eq!(latest_due(&daily(90), second, &denver), Some(first));
}

#[test]
fn half_hour_and_far_east_zones_keep_their_minutes() {
    let kolkata = offsets(rule("<+0530>-5:30"));
    let kiritimati = offsets(rule("<+14>-14"));
    // 08:00 IST on Wednesday is 02:30Z; 08:00 at +14 is 18:00Z Tuesday.
    let ist = WEDNESDAY + 2 * HOUR_MS + 30 * MINUTE_MS;
    let line = WEDNESDAY - 6 * HOUR_MS;

    assert_eq!(latest_due(&daily(8 * 60), ist, &kolkata), Some(ist));
    assert_eq!(latest_due(&daily(8 * 60), line, &kiritimati), Some(line));
}

#[test]
fn a_weekly_plan_finds_its_last_listed_day() {
    let utc = offsets(Rule::UTC);
    // Mondays only (bit 0): from Wednesday the latest is two days back.
    let monday = Plan {
        days: Weekdays(1),
        ..daily(9 * 60)
    };

    assert_eq!(
        weekday(i64::try_from(WEDNESDAY / (24 * HOUR_MS)).unwrap()),
        2
    );
    assert_eq!(
        latest_due(&monday, WEDNESDAY + 12 * HOUR_MS, &utc),
        Some(WEDNESDAY - 2 * 24 * HOUR_MS + 9 * HOUR_MS)
    );
    // Sundays only (bit 6): three days back.
    let sunday = Plan {
        days: Weekdays(1 << 6),
        ..daily(9 * 60)
    };
    assert_eq!(
        latest_due(&sunday, WEDNESDAY + 12 * HOUR_MS, &utc),
        Some(WEDNESDAY - 3 * 24 * HOUR_MS + 9 * HOUR_MS)
    );
}

#[test]
fn a_plan_that_runs_once_occurs_every_day_until_it_runs() {
    let utc = offsets(Rule::UTC);
    let once = Plan {
        days: Weekdays::ONCE,
        ..daily(9 * 60)
    };

    assert_eq!(
        latest_due(&once, WEDNESDAY + 8 * HOUR_MS, &utc),
        Some(WEDNESDAY - 15 * HOUR_MS)
    );
    assert_eq!(
        latest_due(&once, WEDNESDAY + 9 * HOUR_MS, &utc),
        Some(WEDNESDAY + 9 * HOUR_MS)
    );
}

#[test]
fn without_offsets_nothing_is_due() {
    assert_eq!(latest_due(&daily(0), WEDNESDAY, |_| None), None);
    assert_eq!(next_due(&daily(0), WEDNESDAY, |_| None), None);
}

#[test]
fn the_next_occurrence_is_strictly_after_now() {
    let denver = offsets(rule("MST7MDT,M3.2.0,M11.1.0"));
    let plan = daily(7 * 60 + 30);
    // 07:30 MDT on Wednesday is 13:30Z.
    let due = WEDNESDAY + 13 * HOUR_MS + 30 * MINUTE_MS;

    assert_eq!(next_due(&plan, due - 1, &denver), Some(due));
    // At its minute the occurrence is the latest, and the next is tomorrow's.
    assert_eq!(next_due(&plan, due, &denver), Some(due + 24 * HOUR_MS));
}

#[test]
fn the_next_occurrence_follows_the_daylight_rules() {
    let denver = offsets(rule("MST7MDT,M3.2.0,M11.1.0"));
    // The skipped 02:30 runs at 03:30 MDT; the repeated 01:30 on its first
    // pass, so after it the next is the day after's.
    assert_eq!(
        next_due(&daily(2 * 60 + 30), SPRING - HOUR_MS, &denver),
        Some(SPRING + 30 * MINUTE_MS)
    );
    let first = FALL - 30 * MINUTE_MS;
    assert_eq!(next_due(&daily(90), first - 1, &denver), Some(first));
    assert_eq!(
        next_due(&daily(90), first, &denver),
        Some(FALL + 30 * MINUTE_MS + 24 * HOUR_MS)
    );
}

#[test]
fn plans_outside_the_limits_are_invalid() {
    let good = daily(23 * 60 + 59);
    let mut table = [None; PLANS];
    table[3] = Some(good);
    assert!(table_valid(&table));

    for bad in [
        Plan {
            minute: 24 * 60,
            ..good
        },
        Plan {
            portions: 0,
            ..good
        },
        Plan {
            portions: 49,
            ..good
        },
        Plan {
            days: Weekdays(0x80),
            ..good
        },
    ] {
        assert!(!bad.valid(), "{bad:?}");
        table[3] = Some(bad);
        assert!(!table_valid(&table));
    }
}

proptest! {
    /// The latest occurrence is at or before now, on a listed weekday at
    /// the plan's local minute (or past a skipped one), and no later
    /// occurrence fits before now.
    #[test]
    fn the_latest_occurrence_is_the_last_before_now(
        now in 1_700_000_000_000u64..1_900_000_000_000,
        minute in 0u16..1440,
        days in 1u8..0x80,
    ) {
        let denver = offsets(rule("MST7MDT,M3.2.0,M11.1.0"));
        let plan = Plan { minute, portions: 1, days: Weekdays(days), enabled: true, call: 0 };

        let due = latest_due(&plan, now, &denver).unwrap();
        prop_assert!(due <= now);
        prop_assert!(now - due <= 8 * 24 * HOUR_MS);

        let local = i64::try_from(due).unwrap()
            + i64::from(denver(due).unwrap()) * 1000;
        let day = local.div_euclid(86_400_000);
        let local_minute = local.rem_euclid(86_400_000) / 60_000;
        prop_assert!(Weekdays(days).contains(weekday(day)));
        // At the minute, or an hour past it when the change skipped it.
        let skipped = (local_minute - i64::from(minute)) == 60;
        prop_assert!(local_minute == i64::from(minute) || skipped);
    }

    /// The next occurrence is after now, is an occurrence, and none falls
    /// between now and it.
    #[test]
    fn the_next_occurrence_is_the_first_after_now(
        now in 1_700_000_000_000u64..1_900_000_000_000,
        minute in 0u16..1440,
        days in 0u8..0x80,
    ) {
        let denver = offsets(rule("MST7MDT,M3.2.0,M11.1.0"));
        let plan = Plan { minute, portions: 1, days: Weekdays(days), enabled: true, call: 0 };

        let next = next_due(&plan, now, &denver).unwrap();
        prop_assert!(next > now);
        prop_assert!(next - now <= 8 * 24 * HOUR_MS);
        prop_assert_eq!(latest_due(&plan, next, &denver), Some(next));
        prop_assert!(latest_due(&plan, next - 1, &denver).is_none_or(|due| due <= now));
    }
}

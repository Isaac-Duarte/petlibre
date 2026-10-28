//! The desiccant reminder's day count: calendar days of local time from
//! the replacement, on past zero, unknown without a clock, and never moved
//! by a daylight change.

use wellfed_core::clock::wall::Reading;
use wellfed_core::clock::zone::Zone;
use wellfed_core::desiccant::{Count, PERIOD_DEFAULT, PERIOD_MAX, PERIOD_MIN, period_valid, today};

/// US Mountain time: daylight from the second Sunday in March to the first
/// Sunday in November (2026: March 8 and November 1, both at 02:00).
const DENVER: &[u8] = b"MST7MDT,M3.2.0,M11.1.0";

/// A valid reading at `unix_s`, with the zone's offset then.
fn reading(zone: &Zone, unix_s: i64) -> Reading {
    Reading {
        unix_ms: u64::try_from(unix_s * 1000).unwrap(),
        offset_s: Some(zone.rule().offset_at(unix_s)),
        error_ms: 0,
    }
}

/// Unix seconds of a UTC date and time.
fn utc(year: i32, month: u32, day: u32, hour: i64, minute: i64) -> i64 {
    // Days from the civil date (Howard Hinnant's algorithm).
    let (y, m) = if month <= 2 {
        (year - 1, month + 9)
    } else {
        (year, month - 3)
    };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = i64::from(yoe) * 365 + i64::from(yoe / 4) - i64::from(yoe / 100) + i64::from(doy);
    let days = i64::from(era) * 146_097 + doe - 719_468;
    days * 86_400 + hour * 3600 + minute * 60
}

#[test]
fn the_period_is_one_to_ninety_days_and_starts_at_fourteen() {
    assert_eq!((PERIOD_MIN, PERIOD_MAX, PERIOD_DEFAULT), (1, 90, 14));
    assert!(period_valid(1) && period_valid(14) && period_valid(90));
    assert!(!period_valid(0) && !period_valid(91) && !period_valid(u8::MAX));
}

#[test]
fn the_count_runs_from_the_period_through_zero_and_below() {
    let reset = 20_389;
    let count = |today| Count::new(14, Some(reset), Some(today));

    assert_eq!(count(reset), Count::Remaining(14));
    assert_eq!(count(reset + 13), Count::Remaining(1));
    assert_eq!(count(reset + 14), Count::Remaining(0));
    assert_eq!(count(reset + 15), Count::Remaining(-1));
    assert_eq!(count(reset + 400), Count::Remaining(-386));
    assert_eq!(count(reset + 13).due(), Some(false));
    assert_eq!(count(reset + 14).due(), Some(true));
    assert_eq!(count(reset + 400).due(), Some(true));

    // The widest span saturates rather than wrapping to a count left.
    assert_eq!(
        Count::new(90, Some(i32::MIN), Some(i32::MAX)),
        Count::Remaining(i32::MIN)
    );
}

#[test]
fn a_reset_dated_after_today_counts_as_today() {
    assert_eq!(
        Count::new(14, Some(20_400), Some(20_389)),
        Count::Remaining(14)
    );
}

#[test]
fn nothing_recorded_or_no_clock_is_not_a_count() {
    assert_eq!(Count::new(14, None, Some(20_389)), Count::Unset);
    assert_eq!(Count::new(14, None, None), Count::Unset);
    assert_eq!(Count::new(14, Some(20_389), None), Count::Unknown);
    assert_eq!(Count::Unset.due(), None);
    assert_eq!(Count::Unknown.due(), None);
}

#[test]
fn today_is_the_local_calendar_day_or_utc_without_an_offset() {
    // 2026-10-29 03:30 UTC: still the 28th in UTC-6, the 29th in UTC.
    let unix_s = utc(2026, 10, 29, 3, 30);
    let local = Reading {
        unix_ms: u64::try_from(unix_s * 1000).unwrap(),
        offset_s: Some(-6 * 3600),
        error_ms: 0,
    };
    assert_eq!(today(&local), Some(20_754));
    let bare = Reading {
        offset_s: None,
        ..local
    };
    assert_eq!(today(&bare), Some(20_755));

    // East of UTC, the local day starts first.
    let tokyo = Reading {
        unix_ms: u64::try_from(utc(2026, 10, 28, 15, 0) * 1000).unwrap(),
        offset_s: Some(9 * 3600),
        error_ms: 0,
    };
    assert_eq!(today(&tokyo), Some(20_755));
}

/// Across the spring change a day is 23 hours long and across the autumn
/// one 25, yet the count moves by one each local midnight, never by the
/// elapsed hours.
#[test]
fn daylight_changes_never_move_the_count() {
    let zone = Zone::parse(DENVER).unwrap();

    // Replaced at 23:30 MST on Saturday March 7; at 00:30 MDT on Monday
    // March 9 only 24 hours have passed, but two midnights.
    let reset = today(&reading(&zone, utc(2026, 3, 8, 6, 30))).unwrap();
    let monday = today(&reading(&zone, utc(2026, 3, 9, 6, 30))).unwrap();
    assert_eq!(monday - reset, 2);
    assert_eq!(
        Count::new(2, Some(reset), Some(monday)),
        Count::Remaining(0)
    );
    // 23:59 MDT on Sunday March 8 is still one day on.
    let sunday_late = today(&reading(&zone, utc(2026, 3, 9, 5, 59))).unwrap();
    assert_eq!(sunday_late - reset, 1);

    // Replaced at 00:10 MDT on Sunday November 1; at 23:50 MST that night,
    // 24 hours and 40 minutes later, it is still the same day.
    let reset = today(&reading(&zone, utc(2026, 11, 1, 6, 10))).unwrap();
    let same_night = today(&reading(&zone, utc(2026, 11, 2, 6, 50))).unwrap();
    assert_eq!(same_night, reset);
    assert_eq!(
        Count::new(14, Some(reset), Some(same_night)),
        Count::Remaining(14)
    );
    let next_day = today(&reading(&zone, utc(2026, 11, 2, 7, 10))).unwrap();
    assert_eq!(next_day - reset, 1);
}

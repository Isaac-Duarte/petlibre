use wellfed_core::clock::wall::{
    Fix, Jump, LEARNED_FLOOR_PPM, MAX_ERROR_MS, MIN_UNIX_MS, MISSED_GRACE_MS, Occurrence, Origin,
    STEP_MS, SYNC_ERROR_MS, Sync, SyncError, UNLEARNED_PPM, WallClock, occurrence,
};
use wellfed_core::clock::zone::Zone;

// 2026-10-22T13:00:00Z; Mountain Daylight Time until 2026-11-01T08:00:00Z.
const NOON: u64 = 1_792_674_000_000;
const MDT: i32 = -6 * 3600;
const MST: i32 = -7 * 3600;
const FALL_BACK_S: u64 = 1_793_520_000;
const HOUR_MS: u32 = 3_600_000;

fn sync(unix_ms: u64) -> Sync {
    Sync {
        unix_ms,
        offset_s: MDT,
        next_change_s: FALL_BACK_S,
        next_offset_s: MST,
    }
}

fn ntp(unix_ms: u64, error_ms: u32) -> Fix {
    Fix {
        unix_ms,
        error_ms,
        origin: Origin::Ntp,
    }
}

#[test]
fn invalid_until_the_first_sync_then_carried_by_uptime() {
    let mut clock = WallClock::default();
    assert_eq!(clock.read(5_000), None);

    assert_eq!(
        clock.sync(5_000, sync(NOON), Origin::Owner),
        Ok(Jump::First)
    );
    let reading = clock.read(65_000).unwrap();
    assert_eq!(reading.unix_ms, NOON + 60_000);
    assert_eq!(reading.offset_s, Some(MDT));
    assert_eq!(
        reading.local_ms(),
        Some((NOON + 60_000) as i64 - 6 * 3_600_000)
    );
    // The sync's own second plus 2000 ppm of a minute.
    assert_eq!(reading.error_ms, 1_120);
    assert_eq!(clock.syncs(), 1);
}

#[test]
fn an_unlearned_clock_expires_when_its_bound_passes_two_minutes() {
    let mut clock = WallClock::default();
    let start = u32::MAX - 1_000;
    clock.sync(start, sync(NOON), Origin::Owner).unwrap();
    assert_eq!(clock.read(9_000).unwrap().unix_ms, NOON + 10_001);

    // 1 s plus 2000 ppm reaches 120 s after 59 500 s, about 16.5 h.
    let limit =
        u32::try_from((MAX_ERROR_MS - 1000) * 1_000_000 / u64::from(UNLEARNED_PPM)).unwrap();
    assert!(clock.read(start.wrapping_add(limit)).is_some());
    assert_eq!(clock.read(start.wrapping_add(limit + 1000)), None);
}

#[test]
fn ntp_alone_sets_an_invalid_clock_without_an_offset() {
    let mut clock = WallClock::default();
    assert_eq!(clock.fix(0, ntp(NOON, 40)), Ok(Jump::First));
    let reading = clock.read(0).unwrap();
    assert_eq!((reading.unix_ms, reading.error_ms), (NOON, 40));
    assert_eq!(reading.offset_s, None);
    assert_eq!(reading.local_ms(), None);
}

#[test]
fn the_offset_follows_the_announced_change_or_the_zone_rule() {
    let mut clock = WallClock::default();
    let before = FALL_BACK_S * 1000 - 1000;
    clock.sync(0, sync(before), Origin::Owner).unwrap();
    assert_eq!(clock.read(999).unwrap().offset_s, Some(MDT));
    assert_eq!(clock.read(1000).unwrap().offset_s, Some(MST));

    // A rule wins over the sync's hint: New York left daylight time two
    // hours before Denver.
    let rule = Zone::parse(b"EST5EDT,M3.2.0,M11.1.0").unwrap().rule();
    clock.set_rule(Some(rule));
    assert_eq!(clock.read(999).unwrap().offset_s, Some(-5 * 3600));
    clock.set_rule(None);
    assert_eq!(clock.read(999).unwrap().offset_s, Some(MDT));
}

#[test]
fn owner_syncs_step_freely_and_are_classified() {
    let mut clock = WallClock::default();
    clock.sync(0, sync(NOON), Origin::Owner).unwrap();
    assert_eq!(
        clock.sync(10_000, sync(NOON + 10_000 + STEP_MS as u64), Origin::Owner),
        Ok(Jump::Step(STEP_MS))
    );
    assert_eq!(
        clock.sync(20_000, sync(NOON + 20_000 + 3_600_000), Origin::Owner),
        Ok(Jump::Jumped(3_600_000 - STEP_MS))
    );
    assert_eq!(
        clock.sync(30_000, sync(NOON), Origin::Owner),
        Ok(Jump::Jumped(-3_630_000))
    );
    assert_eq!(clock.read(30_000).unwrap().unix_ms, NOON);
}

#[test]
fn other_sources_only_narrow_a_valid_clock() {
    let mut clock = WallClock::default();
    clock.sync(0, sync(NOON), Origin::Owner).unwrap();

    // Ten minutes on the clock reads NOON+600 000 ± 2 200 ms. NTP 1.5 s
    // ahead with 100 ms of error lies inside: the overlap is
    // [NOON+601 400, NOON+601 600].
    let now = 600_000;
    let jump = clock.fix(now, ntp(NOON + 601_500, 100)).unwrap();
    let reading = clock.read(now).unwrap();
    assert_eq!(reading.unix_ms, NOON + 601_500);
    assert_eq!(reading.error_ms, 100);
    assert_eq!(jump, Jump::Step(1_500));

    // An hour off is refused, and the clock is unchanged.
    assert_eq!(
        clock.fix(now, ntp(NOON + 600_000 + 3_600_000, 100)),
        Err(SyncError::Bounded)
    );
    assert_eq!(
        clock.sync(now, sync(NOON + 600_000 + 3_600_000), Origin::Client),
        Err(SyncError::Bounded)
    );
    assert_eq!(clock.read(now).unwrap().unix_ms, NOON + 601_500);
    assert_eq!(clock.syncs(), 2);
}

#[test]
fn a_member_cannot_walk_the_clock_with_many_syncs() {
    let mut clock = WallClock::default();
    clock.sync(0, sync(NOON), Origin::Owner).unwrap();

    // Each sync claims a time just inside the clock's interval, ahead.
    let mut now = 0;
    for _ in 0..1000 {
        now += 100;
        let reading = clock.read(now).unwrap();
        let claimed = reading.unix_ms + reading.error_ms + u64::from(SYNC_ERROR_MS);
        let _ = clock.sync(now, sync(claimed), Origin::Client);
    }
    // 100 s passed; the clock may sit at most its own initial error plus
    // the drift bound ahead of true time.
    let truth = NOON + u64::from(now);
    let reading = clock.read(now).unwrap();
    let ahead = reading.unix_ms - truth;
    assert!(ahead <= 1_000 + 200 + 1, "walked {ahead} ms");
}

#[test]
fn an_expired_clock_takes_any_fix() {
    let mut clock = WallClock::default();
    clock.sync(0, sync(NOON), Origin::Owner).unwrap();
    let later = 20 * HOUR_MS;
    assert_eq!(clock.read(later), None);
    assert_eq!(
        clock.fix(later, ntp(NOON + 999_999_999, 50)),
        Ok(Jump::First)
    );
}

/// An uptime clock that runs 735 ppm slow, as the research unit's did.
fn slow(true_elapsed_ms: u64) -> u32 {
    u32::try_from(true_elapsed_ms - true_elapsed_ms * 735 / 1_000_000).unwrap()
}

#[test]
fn the_rate_is_learned_and_the_clock_holds_for_days() {
    let mut clock = WallClock::default();
    clock.fix(0, ntp(NOON, 20)).unwrap();
    assert!(!clock.state(0).learned);

    // Fixes every 17 minutes for two hours: the baseline passes the
    // 50 ppm measurement after about 27 minutes.
    let step = 1_024_000;
    for i in 1..=7u64 {
        let truth = i * step;
        clock.fix(slow(truth), ntp(NOON + truth, 20)).unwrap();
    }
    let state = clock.state(slow(7 * step));
    assert!(state.learned);
    assert!(
        (state.rate_ppb - 735_000).abs() < 1_000,
        "{}",
        state.rate_ppb
    );
    assert_eq!(state.rate_bound_ppb, LEARNED_FLOOR_PPM * 1000);

    // Ten days with no source: the corrected clock is within a second of
    // the truth, and still valid with a bound under two minutes.
    let truth = 7 * step + 10 * 24 * u64::from(HOUR_MS);
    let reading = clock.read(slow(truth)).unwrap();
    let off = reading.unix_ms.abs_diff(NOON + truth);
    assert!(off < 1_000, "off by {off} ms");
    assert!(reading.error_ms <= MAX_ERROR_MS);

    // Fifteen days is past the bound.
    let truth = 7 * step + 15 * 24 * u64::from(HOUR_MS);
    assert_eq!(clock.read(slow(truth)), None);
}

#[test]
fn an_owner_jump_restarts_the_baseline() {
    let mut clock = WallClock::default();
    clock.fix(0, ntp(NOON, 20)).unwrap();
    clock
        .sync(60_000, sync(NOON + 3_600_000), Origin::Owner)
        .unwrap();
    // Measured against the jump's own fix, nothing is learned yet.
    clock.fix(61_000, ntp(NOON + 3_601_000, 20)).unwrap();
    assert!(!clock.state(61_000).learned);
}

#[test]
fn bad_syncs_are_refused_and_leave_the_clock() {
    let mut clock = WallClock::default();
    clock.sync(0, sync(NOON), Origin::Owner).unwrap();
    let cases = [
        (
            Sync {
                unix_ms: MIN_UNIX_MS - 1,
                ..sync(NOON)
            },
            SyncError::Time,
        ),
        (
            Sync {
                offset_s: 15 * 3600,
                ..sync(NOON)
            },
            SyncError::Offset,
        ),
        (
            Sync {
                next_offset_s: -13 * 3600,
                ..sync(NOON)
            },
            SyncError::Offset,
        ),
        (
            Sync {
                next_change_s: NOON / 1000,
                ..sync(NOON)
            },
            SyncError::NextChange,
        ),
    ];
    for (bad, error) in cases {
        assert_eq!(clock.sync(1_000, bad, Origin::Owner), Err(error));
    }
    assert_eq!(
        clock.fix(1_000, ntp(MIN_UNIX_MS - 1, 1)),
        Err(SyncError::Time)
    );
    assert_eq!(clock.read(1_000).unwrap().unix_ms, NOON + 1_000);
    assert_eq!(clock.syncs(), 1);

    // No announced change is allowed.
    assert!(
        clock
            .sync(
                2_000,
                Sync {
                    next_change_s: 0,
                    ..sync(NOON + 2_000)
                },
                Origin::Owner
            )
            .is_ok()
    );
}

#[test]
fn occurrences_run_once_and_late_ones_are_missed() {
    let due = NOON;
    assert_eq!(occurrence(due, None, due - 1), Occurrence::Later);
    assert_eq!(occurrence(due, None, due), Occurrence::Run);
    assert_eq!(
        occurrence(due, Some(due - 1), due + MISSED_GRACE_MS),
        Occurrence::Run
    );
    // A forward jump or a restart past the grace window.
    assert_eq!(
        occurrence(due, Some(due - 1), due + MISSED_GRACE_MS + 1),
        Occurrence::Missed
    );
    // A backward jump re-enters evaluated time: never twice.
    assert_eq!(
        occurrence(due, Some(due + 5_000), due - 3_600_000),
        Occurrence::Evaluated
    );
    assert_eq!(occurrence(due, Some(due), due + 1), Occurrence::Evaluated);
}

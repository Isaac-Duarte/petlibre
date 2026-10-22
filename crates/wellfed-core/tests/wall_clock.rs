use wellfed_core::wall_clock::{
    Jump, MAX_UNCERTAINTY_MS, MIN_UNIX_MS, MISSED_GRACE_MS, Occurrence, STEP_MS, Sync, SyncError,
    WallClock, occurrence,
};

// 2026-10-22T13:00:00Z; Mountain Daylight Time until 2026-11-01T08:00:00Z.
const NOON: u64 = 1_792_674_000_000;
const MDT: i32 = -6 * 3600;
const MST: i32 = -7 * 3600;
const FALL_BACK_S: u64 = 1_793_520_000;

fn sync(unix_ms: u64) -> Sync {
    Sync {
        unix_ms,
        offset_s: MDT,
        next_change_s: FALL_BACK_S,
        next_offset_s: MST,
    }
}

#[test]
fn invalid_until_the_first_sync_then_carried_by_uptime() {
    let mut clock = WallClock::default();
    assert_eq!(clock.read(5_000), None);

    assert_eq!(clock.sync(5_000, sync(NOON)), Ok(Jump::First));
    let reading = clock.read(65_000).unwrap();
    assert_eq!(reading.unix_ms, NOON + 60_000);
    assert_eq!(reading.offset_s, MDT);
    assert_eq!(reading.local_ms(), (NOON + 60_000) as i64 - 6 * 3_600_000);
    assert!(reading.uncertainty_ms >= 1000 && reading.uncertainty_ms < 1200);
    assert_eq!(clock.syncs(), 1);
}

#[test]
fn uptime_wraps_and_the_clock_expires_with_drift() {
    let mut clock = WallClock::default();
    let start = u32::MAX - 1_000;
    clock.sync(start, sync(NOON)).unwrap();
    assert_eq!(clock.read(9_000).unwrap().unix_ms, NOON + 10_001);

    // 2000 ppm plus the 1 s allowance reaches 60 s after 29 500 s.
    let limit = ((MAX_UNCERTAINTY_MS - 1000) * 500) as u32;
    assert!(clock.read(start.wrapping_add(limit)).is_some());
    assert_eq!(clock.read(start.wrapping_add(limit + 1000)), None);
}

#[test]
fn the_offset_follows_the_announced_change() {
    let mut clock = WallClock::default();
    let before = FALL_BACK_S * 1000 - 1000;
    clock.sync(0, sync(before)).unwrap();
    assert_eq!(clock.read(999).unwrap().offset_s, MDT);
    assert_eq!(clock.read(1000).unwrap().offset_s, MST);
}

#[test]
fn syncs_are_classified_against_the_running_estimate() {
    let mut clock = WallClock::default();
    clock.sync(0, sync(NOON)).unwrap();
    assert_eq!(
        clock.sync(10_000, sync(NOON + 10_000 + STEP_MS as u64)),
        Ok(Jump::Step(STEP_MS))
    );
    assert_eq!(
        clock.sync(20_000, sync(NOON + 20_000 + 3_600_000)),
        Ok(Jump::Jumped(3_600_000 - STEP_MS))
    );
    assert_eq!(clock.sync(30_000, sync(NOON)), Ok(Jump::Jumped(-3_630_000)));
}

#[test]
fn bad_syncs_are_refused_and_leave_the_clock() {
    let mut clock = WallClock::default();
    clock.sync(0, sync(NOON)).unwrap();
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
        assert_eq!(clock.sync(1_000, bad), Err(error));
    }
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
                }
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

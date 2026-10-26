use wellfed_core::clock::wall::{
    CARRY_MAX_S, CARRY_WORDS, DISJOINT_LIMIT, Fix, Jump, LEARNED_FLOOR_PPM, MAX_ERROR_MS,
    MIN_UNIX_MS, MISSED_GRACE_MS, NoResume, Occurrence, Origin, Reading, Restart, Resumed, STEP_MS,
    SYNC_ERROR_MS, Sync, SyncError, UNLEARNED_PPM, WallClock, occurrence,
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

fn fix(unix_ms: u64, error_ms: u32, origin: Origin) -> Fix {
    Fix {
        unix_ms,
        error_ms,
        origin,
    }
}

fn ntp(unix_ms: u64, error_ms: u32) -> Fix {
    fix(unix_ms, error_ms, Origin::Ntp)
}

/// A valid reading never claims to be closer to `truth` than it is.
fn honest(reading: Reading, truth: u64, when: &str) {
    let off = reading.unix_ms.abs_diff(truth);
    assert!(
        off <= reading.error_ms,
        "{when}: {off} ms off, claims {} ms",
        reading.error_ms
    );
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
    // The sync's own second plus 5000 ppm of a minute.
    assert_eq!(reading.error_ms, 1_300);
    assert_eq!(clock.syncs(), 1);
}

#[test]
fn an_unlearned_clock_expires_when_its_bound_passes_two_minutes() {
    let mut clock = WallClock::default();
    let start = u32::MAX - 1_000;
    clock.sync(start, sync(NOON), Origin::Owner).unwrap();
    assert_eq!(clock.read(9_000).unwrap().unix_ms, NOON + 10_001);

    // 1 s plus 5000 ppm reaches 120 s after 23 800 s, about 6.6 h.
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
fn other_sources_move_the_reading_only_within_the_trusted_interval() {
    let mut clock = WallClock::default();
    clock.sync(0, sync(NOON), Origin::Owner).unwrap();

    // Ten minutes on, the trusted interval is NOON+600 000 ± 4 000 ms. NTP
    // 1.5 s ahead with 100 ms of error lies inside: the reading moves to
    // it, but still claims the trusted interval's error and its own move.
    let now = 600_000;
    let jump = clock.fix(now, ntp(NOON + 601_500, 100)).unwrap();
    let reading = clock.read(now).unwrap();
    assert_eq!(reading.unix_ms, NOON + 601_500);
    assert_eq!(reading.error_ms, 1_500 + 4_000);
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
    // 100 s passed; the clock may sit at most the trusted interval's error
    // (the owner's second plus drift) ahead of true time, and says so.
    let truth = NOON + u64::from(now);
    let reading = clock.read(now).unwrap();
    assert!(reading.unix_ms - truth <= 1_000 + 500 + 1);
    honest(reading, truth, "after 1000 syncs");
}

/// The reviewer's walk: an attacker answers the feeder's NTP every hour at
/// the edge of what the clock allows, with a small claimed error, and an
/// honest answer follows. With an owner's sync every twelve hours the
/// clock never leaves the trusted interval and never claims to know more
/// than it does, for a week.
#[test]
fn an_ntp_attacker_cannot_walk_a_valid_clock_past_its_bound() {
    let mut clock = WallClock::default();
    clock.fix(0, fix(NOON, 50, Origin::Owner)).unwrap();
    for hour in 1..=24 * 7u32 {
        let now = hour * HOUR_MS;
        let truth = NOON + u64::from(now);
        if hour % 12 == 0 {
            clock.fix(now, fix(truth, 50, Origin::Owner)).unwrap();
        }
        let reading = clock.read(now).unwrap();
        let edge = reading.unix_ms + reading.error_ms - 5;
        let _ = clock.fix(now, ntp(edge, 5));
        let _ = clock.fix(now + 1000, ntp(truth + 1000, 20));

        let reading = clock.read(now + 1000).expect("valid with owner syncs");
        honest(reading, truth + 1000, &format!("hour {hour}"));
        assert!(reading.error_ms <= MAX_ERROR_MS);
    }
}

/// Without an owner's sync, the same attack runs into the clock's expiry:
/// it reads invalid once what it claims passes two minutes, instead of
/// staying valid with a small error while walked away. (Pushed to the edge
/// of the trusted interval, it claims the push as well, so it expires
/// sooner than the trusted interval alone would.)
#[test]
fn without_an_owner_the_attacked_clock_expires_rather_than_lie() {
    let mut clock = WallClock::default();
    clock.fix(0, fix(NOON, 50, Origin::Owner)).unwrap();
    let mut expired = None;
    for hour in 1..=24u32 {
        let now = hour * HOUR_MS;
        let truth = NOON + u64::from(now);
        let reading = clock.read(now).expect("valid until pushed past its bound");
        honest(reading, truth, &format!("hour {hour}"));
        let _ = clock.fix(now, ntp(reading.unix_ms + reading.error_ms - 5, 5));
        match clock.read(now) {
            Some(reading) => honest(reading, truth, &format!("hour {hour}, pushed")),
            None => {
                expired = Some(hour);
                break;
            }
        }
        let _ = clock.fix(now + 1000, ntp(truth + 1000, 20));
    }
    assert!(expired.is_some_and(|hour| hour <= 7), "{expired:?}");
}

/// The reviewer's poisoning: one spoofed answer an hour after the first
/// NTP fix must not teach the clock a rate. Every reading afterwards stays
/// honest, and nothing is learned from untrusted fixes.
#[test]
fn a_spoofed_answer_does_not_teach_the_clock_a_rate() {
    let mut clock = WallClock::default();
    clock.fix(0, ntp(NOON, 20)).unwrap();
    let span = HOUR_MS;
    let reading = clock.read(span).unwrap();
    clock
        .fix(span, ntp(reading.unix_ms + reading.error_ms - 25, 20))
        .unwrap();
    assert!(!clock.state(span).learned);
    assert_eq!(clock.state(span).rate_ppb, 0);

    for hours in [1u32, 6, 24, 72, 240] {
        let now = span + hours * HOUR_MS;
        let truth = NOON + u64::from(now);
        if let Some(reading) = clock.read(now) {
            honest(reading, truth, &format!("+{hours} h"));
        }
        let _ = clock.fix(now, ntp(truth, 20));
        let reading = clock.read(now).expect("an honest answer sets it");
        honest(reading, truth, &format!("+{hours} h, answered"));
    }
}

/// An uptime clock that runs `ppm` slow.
fn slow_by(ppm: u64, true_elapsed_ms: u64) -> u32 {
    u32::try_from(true_elapsed_ms - true_elapsed_ms * ppm / 1_000_000).unwrap()
}

/// An uptime clock that runs 735 ppm slow, as the research unit's did on
/// October 22.
fn slow(true_elapsed_ms: u64) -> u32 {
    slow_by(735, true_elapsed_ms)
}

/// The bench's refusal (October 26): with the uptime clock 2,060 ppm slow,
/// the poll 1,026 s after the first NTP fix fell outside a 2,000 ppm
/// interval. It falls well inside the unlearned bound now, and the clock
/// stays honest across a day of polls.
#[test]
fn ntp_polls_stay_inside_the_bound_at_the_measured_worst_drift() {
    let mut clock = WallClock::default();
    clock.fix(0, ntp(NOON, 25)).unwrap();

    let step = 1_026_000u64;
    for i in 1..=84u64 {
        let truth = i * step;
        let now = slow_by(2060, truth);
        if let Some(reading) = clock.read(now) {
            honest(reading, NOON + truth, &format!("poll {i}"));
            assert!(clock.fix(now, ntp(NOON + truth, 25)).is_ok(), "poll {i}");
        } else {
            assert_eq!(clock.fix(now, ntp(NOON + truth, 25)), Ok(Jump::First));
        }
    }
    let state = clock.state(slow_by(2060, 84 * step));
    assert!(state.learned);
    assert!(
        (state.rate_ppb - 2_064_000).abs() < 5_000,
        "{}",
        state.rate_ppb
    );
}

#[test]
fn the_rate_is_learned_from_owner_syncs_and_the_clock_holds_for_a_day() {
    let mut clock = WallClock::default();
    clock.fix(0, fix(NOON, 20, Origin::Owner)).unwrap();
    assert!(!clock.state(0).learned);

    // Owner fixes every 17 minutes for two hours: the baseline passes the
    // 50 ppm measurement after about 27 minutes.
    let step = 1_024_000;
    for i in 1..=7u64 {
        let truth = i * step;
        clock
            .fix(slow(truth), fix(NOON + truth, 20, Origin::Owner))
            .unwrap();
    }
    let state = clock.state(slow(7 * step));
    assert!(state.learned);
    assert!(
        (state.rate_ppb - 735_000).abs() < 1_000,
        "{}",
        state.rate_ppb
    );
    assert_eq!(state.rate_bound_ppb, LEARNED_FLOOR_PPM * 1000);

    // 21 hours with no source: the corrected clock is within a second of
    // the truth, and still valid with a bound under two minutes.
    let truth = 7 * step + 21 * u64::from(HOUR_MS);
    let reading = clock.read(slow(truth)).unwrap();
    let off = reading.unix_ms.abs_diff(NOON + truth);
    assert!(off < 1_000, "off by {off} ms");
    assert!(reading.error_ms <= MAX_ERROR_MS);

    // 23 hours is past the learned floor's 22.
    let truth = 7 * step + 23 * u64::from(HOUR_MS);
    assert_eq!(clock.read(slow(truth)), None);
}

#[test]
fn ntp_alone_learns_the_rate_across_its_first_expiry() {
    let mut clock = WallClock::default();
    clock.fix(0, ntp(NOON, 20)).unwrap();

    // Answers every 17 minutes keep the reading on time but teach nothing;
    // after about 6 h the trusted interval expires, and the next answer is
    // trusted, measuring the rate across the whole span.
    let step = 1_024_000u64;
    let mut learned_at = None;
    for i in 1..=80u64 {
        let truth = i * step;
        let now = slow(truth);
        if let Some(reading) = clock.read(now) {
            honest(reading, NOON + truth, &format!("poll {i}"));
        }
        let _ = clock.fix(now, ntp(NOON + truth, 20));
        if learned_at.is_none() && clock.state(now).learned {
            learned_at = Some(i);
        }
    }
    // The reading follows NTP while the trusted estimate runs 735 ppm
    // slow; the gap counts in the error, so it expires after about 5.8 h.
    let i = learned_at.expect("learned after the first expiry");
    assert!((20..=22).contains(&i), "learned at poll {i}");
    let state = clock.state(slow(80 * step));
    assert!(
        (state.rate_ppb - 735_000).abs() < 1_000,
        "{}",
        state.rate_ppb
    );
}

#[test]
fn an_owner_sync_that_disagrees_forgets_the_rate() {
    let mut clock = WallClock::default();
    clock.fix(0, fix(NOON, 20, Origin::Owner)).unwrap();
    clock
        .fix(slow(7_200_000), fix(NOON + 7_200_000, 20, Origin::Owner))
        .unwrap();
    assert!(clock.state(slow(7_200_000)).learned);

    let now = slow(7_300_000);
    clock
        .fix(now, fix(NOON + 7_300_000 + 3_600_000, 20, Origin::Owner))
        .unwrap();
    let state = clock.state(now);
    assert!(!state.learned);
    assert_eq!(state.rate_bound_ppb, UNLEARNED_PPM * 1000);
}

#[test]
fn ntp_answers_that_keep_missing_the_clock_make_it_need_the_time() {
    let mut clock = WallClock::default();
    clock.fix(0, fix(NOON, 20, Origin::Owner)).unwrap();
    let off = NOON + 3_600_000;
    for n in 1..DISJOINT_LIMIT {
        assert_eq!(
            clock.fix(u32::from(n) * 1000, ntp(off, 20)),
            Err(SyncError::Bounded)
        );
        assert!(clock.read(u32::from(n) * 1000).is_some());
    }
    // A client's disagreement does not count towards it.
    assert_eq!(
        clock.fix(5_000, fix(off, 20, Origin::Client)),
        Err(SyncError::Bounded)
    );
    assert!(clock.read(5_000).is_some());
    assert_eq!(clock.fix(6_000, ntp(off, 20)), Err(SyncError::Bounded));
    assert_eq!(clock.read(6_000), None, "the clock needs the time");
    assert_eq!(clock.fix(7_000, ntp(off, 20)), Ok(Jump::First));
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

/// An owner's sync at uptime 0, saved a minute later at RTC second 1000.
fn saved() -> [u32; CARRY_WORDS] {
    let mut clock = WallClock::default();
    clock.sync(0, sync(NOON), Origin::Owner).unwrap();
    clock.carry(60_000, 1000).expect("valid a minute on")
}

#[test]
fn a_warm_reset_resumes_the_clock_with_the_rtc_gap_in_its_error() {
    let mut clock = WallClock::default();

    // 12 RTC seconds later, 3 s into the new boot. The saved error is the
    // sync's second plus 5000 ppm of a minute; the gap adds the RTC's
    // second of resolution and 1 % of 13 s.
    let resumed = clock.resume(3_000, &saved(), Some(1012), Restart::Warm);
    assert_eq!(
        resumed,
        Ok(Resumed {
            gap_s: 12,
            error_ms: 1_300 + 1_000 + 130,
        })
    );

    let reading = clock.read(3_000).unwrap();
    assert_eq!(reading.unix_ms, NOON + 72_000);
    assert_eq!(reading.offset_s, Some(MDT));
    for truth in [NOON + 71_000, NOON + 73_000] {
        honest(reading, truth, "either end of the RTC's second");
    }

    // Carried on by uptime from there, unlearned, with no sync counted.
    let state = clock.state(63_000);
    assert_eq!(state.reading.unwrap().error_ms, 2_430 + 300);
    assert!(!state.learned);
    assert_eq!((state.syncs, state.last), (0, None));
}

#[test]
fn a_resumed_clock_keeps_the_trusted_interval_and_learns_no_rate_from_it() {
    let mut clock = WallClock::default();
    clock
        .resume(3_000, &saved(), Some(1012), Restart::Warm)
        .unwrap();

    // Still trusted: NTP an hour off is refused, inside it only narrows.
    assert_eq!(
        clock.fix(4_000, ntp(NOON + 73_000 + 3_600_000, 20)),
        Err(SyncError::Bounded)
    );
    assert_eq!(
        clock.fix(4_000, ntp(NOON + 73_500, 20)),
        Ok(Jump::Step(500))
    );
    assert!(clock.read(4_000).unwrap().error_ms >= 2_430);

    // The resumed anchor is no baseline: one owner sync after it teaches
    // nothing, however far apart.
    clock
        .fix(4_000 + 7_200_000, fix(NOON + 7_273_500, 20, Origin::Owner))
        .unwrap();
    assert!(!clock.state(4_000 + 7_200_000).learned);
}

#[test]
fn nothing_resumes_after_a_power_cut() {
    let mut clock = WallClock::default();
    let record = saved();

    assert_eq!(
        clock.resume(3_000, &record, Some(1012), Restart::Cold),
        Err(NoResume::Cold)
    );
    // A power cut the reset cause missed: the RTC restarted at 0.
    assert_eq!(
        clock.resume(3_000, &record, Some(4), Restart::Warm),
        Err(NoResume::Backwards)
    );
    // Memory a power-on left.
    assert_eq!(
        clock.resume(3_000, &[0; CARRY_WORDS], Some(1012), Restart::Warm),
        Err(NoResume::Empty)
    );
    assert_eq!(
        clock.resume(
            3_000,
            &[0xa5a5_a5a5; CARRY_WORDS],
            Some(1012),
            Restart::Warm
        ),
        Err(NoResume::Empty)
    );
    assert_eq!(clock.read(3_000), None);
}

#[test]
fn a_stale_or_corrupt_record_is_not_resumed() {
    let mut clock = WallClock::default();
    let record = saved();

    let gap = 1000 + CARRY_MAX_S + 1;
    assert_eq!(
        clock.resume(3_000, &record, Some(gap), Restart::Warm),
        Err(NoResume::Stale)
    );
    assert_eq!(
        clock.resume(3_000, &record, None, Restart::Warm),
        Err(NoResume::Rtc)
    );

    // Any flipped bit fails the check, the check word's included.
    for word in 1..CARRY_WORDS {
        let mut bad = record;
        bad[word] ^= 1 << (word % 32);
        assert_eq!(
            clock.resume(3_000, &bad, Some(1012), Restart::Warm),
            Err(NoResume::Corrupt),
            "word {word}"
        );
    }
    assert_eq!(clock.read(3_000), None);
}

#[test]
fn a_record_too_close_to_expiry_is_not_resumed() {
    let mut clock = WallClock::default();
    clock.sync(0, sync(NOON), Origin::Owner).unwrap();
    // 118.5 s of error at 23 500 s; the gap's second and more pass 120 s.
    let record = clock.carry(23_500_000, 50_000).unwrap();
    assert!(clock.carry(23_900_000, 50_000).is_none(), "invalid by then");

    let mut fresh = WallClock::default();
    assert_eq!(
        fresh.resume(1_000, &record, Some(50_100), Restart::Warm),
        Err(NoResume::Expired)
    );
    assert_eq!(fresh.read(1_000), None);
}

#[test]
fn a_clock_with_the_time_is_not_overwritten_by_a_record() {
    let mut clock = WallClock::default();
    clock.fix(0, ntp(NOON + 3_600_000, 20)).unwrap();
    assert_eq!(
        clock.resume(0, &saved(), Some(1012), Restart::Warm),
        Err(NoResume::Set)
    );
    assert_eq!(clock.read(0).unwrap().unix_ms, NOON + 3_600_000);
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

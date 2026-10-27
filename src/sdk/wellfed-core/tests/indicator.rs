//! The indicator service: per light the highest priority wins and a tie
//! goes to the newer request; a cancelled or expired winner reveals the
//! next; stale handles cancel nothing; blinks keep their phase across the
//! clock's wrap; and the driver writes each change once, checks it after
//! it settles, and turns every light off for good on a failure.

use wellfed_core::indicator::{
    Check, Claim, Drive, Failure, Frame, Indicators, Light, Lights, MIN_PHASE_MS, Pattern,
    Priority, Refused, Request,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Low,
    Mid,
    High,
}

type Table = Indicators<State, 4>;

fn request(light: Light, pattern: Pattern, priority: u8, state: State) -> Request<State> {
    Request {
        light,
        pattern,
        priority: Priority(priority),
        lasts_ms: None,
        state,
    }
}

const BLINK: Pattern = Pattern::Blink {
    on_ms: 100,
    off_ms: 300,
};

fn lit(frame: Frame, light: Light) -> bool {
    frame.levels[light.index()]
}

#[test]
fn the_highest_priority_wins_and_a_cancel_reveals_the_next() {
    let mut table = Table::new();
    assert_eq!(table.frame(0).levels, [false; Light::COUNT]);
    assert_eq!(table.frame(0).next_ms, u32::MAX);

    let low = table
        .show(0, request(Light::Lock, Pattern::Steady, 10, State::Low))
        .unwrap();
    let high = table
        .show(0, request(Light::Lock, Pattern::Off, 90, State::High))
        .unwrap();
    let mid = table
        .show(0, request(Light::Lock, BLINK, 50, State::Mid))
        .unwrap();

    // The high Off request holds the light dark over both.
    assert!(!lit(table.frame(0), Light::Lock));
    assert_eq!(table.winner(Light::Lock).unwrap().state, State::High);

    assert!(table.cancel(high));
    assert_eq!(table.winner(Light::Lock).unwrap().state, State::Mid);
    assert!(lit(table.frame(0), Light::Lock));

    assert!(table.cancel(mid));
    let frame = table.frame(5);
    assert!(lit(frame, Light::Lock));
    assert_eq!(frame.next_ms, u32::MAX, "a steady light never changes");

    assert!(table.cancel(low));
    assert!(table.winner(Light::Lock).is_none());
    assert!(!lit(table.frame(5), Light::Lock));
}

#[test]
fn lights_are_independent_and_a_tie_goes_to_the_newer_request() {
    let mut table = Table::new();
    let first = table
        .show(0, request(Light::Fault, Pattern::Off, 20, State::Low))
        .unwrap();
    table
        .show(10, request(Light::Fault, Pattern::Steady, 20, State::Mid))
        .unwrap();
    table
        .show(10, request(Light::Food, BLINK, 1, State::High))
        .unwrap();

    let frame = table.frame(20);
    assert!(lit(frame, Light::Fault));
    assert!(lit(frame, Light::Food));
    assert!(!lit(frame, Light::Lock) && !lit(frame, Light::Connection));
    assert_eq!(table.winner(Light::Fault).unwrap().state, State::Mid);

    // The older equal request is still underneath.
    assert!(table.holds(first));
    assert_eq!(table.winner(Light::Food).unwrap().state, State::High);
}

#[test]
fn a_stale_handle_cancels_nothing_even_after_its_slot_is_reused() {
    let mut table = Table::new();
    let old = table
        .show(0, request(Light::Lock, Pattern::Steady, 1, State::Low))
        .unwrap();
    assert!(table.cancel(old));
    assert!(!table.cancel(old), "a second cancel finds nothing");

    let new = table
        .show(0, request(Light::Lock, Pattern::Steady, 1, State::Mid))
        .unwrap();
    assert!(
        !table.cancel(old),
        "the reused slot belongs to the new request"
    );
    assert!(table.holds(new));
    assert_eq!(table.winner(Light::Lock).unwrap().state, State::Mid);
}

#[test]
fn an_expired_request_ends_by_itself_and_reveals_the_next() {
    let mut table = Table::new();
    table
        .show(0, request(Light::Fault, Pattern::Steady, 10, State::Low))
        .unwrap();
    let flash = table
        .show(
            1_000,
            Request {
                lasts_ms: Some(3_000),
                ..request(Light::Fault, Pattern::Off, 70, State::High)
            },
        )
        .unwrap();

    let frame = table.frame(1_000);
    assert!(!lit(frame, Light::Fault));
    assert_eq!(frame.next_ms, 3_000, "the expiry is the next change");
    assert_eq!(table.frame(3_999).next_ms, 1);
    assert!(table.holds(flash));

    let frame = table.frame(4_000);
    assert!(lit(frame, Light::Fault));
    assert!(!table.holds(flash));
    assert!(!table.cancel(flash));
    assert_eq!(table.winner(Light::Fault).unwrap().state, State::Low);
}

#[test]
fn an_expiry_across_the_clock_wrap_still_ends_on_time() {
    let mut table = Table::new();
    let start = u32::MAX - 500;
    table
        .show(
            start,
            Request {
                lasts_ms: Some(1_000),
                ..request(Light::Food, Pattern::Steady, 1, State::Low)
            },
        )
        .unwrap();
    assert!(lit(table.frame(start.wrapping_add(999)), Light::Food));
    assert!(!lit(table.frame(start.wrapping_add(1_000)), Light::Food));
}

#[test]
fn a_blink_starts_lit_and_reports_each_edge() {
    let mut table = Table::new();
    table
        .show(1_000, request(Light::Connection, BLINK, 1, State::Low))
        .unwrap();

    let expect = [
        (1_000, true, 100),
        (1_050, true, 50),
        (1_100, false, 300),
        (1_399, false, 1),
        (1_400, true, 100),
        (1_500, false, 300),
        // A late pass lands in the right phase of a later period.
        (5_050, true, 50),
    ];
    for (now, level, next) in expect {
        let frame = table.frame(now);
        assert_eq!(lit(frame, Light::Connection), level, "at {now}");
        assert_eq!(frame.next_ms, next, "at {now}");
    }
}

#[test]
fn a_blink_keeps_its_phase_across_the_clock_wrap() {
    let mut table = Table::new();
    let mut now = 0u32;
    table
        .show(now, request(Light::Lock, BLINK, 1, State::Low))
        .unwrap();

    // Passes every 7.3 s for longer than the clock's range: the phase is
    // always (elapsed since the start) mod 400, in true elapsed time.
    let mut elapsed: u64 = 0;
    for _ in 0..600_000 {
        now = now.wrapping_add(7_300);
        elapsed += 7_300;
        let phase = elapsed % 400;
        assert_eq!(
            lit(table.frame(now), Light::Lock),
            phase < 100,
            "at {elapsed}"
        );
    }
    assert!(elapsed > u64::from(u32::MAX));
}

#[test]
fn bad_patterns_durations_and_a_full_table_are_refused() {
    let mut table = Table::new();
    let short = Pattern::Blink {
        on_ms: MIN_PHASE_MS - 1,
        off_ms: 500,
    };
    assert_eq!(
        table.show(0, request(Light::Lock, short, 1, State::Low)),
        Err(Refused::Pattern)
    );
    for lasts in [0, 0x8000_0000] {
        assert_eq!(
            table.show(
                0,
                Request {
                    lasts_ms: Some(lasts),
                    ..request(Light::Lock, Pattern::Steady, 1, State::Low)
                }
            ),
            Err(Refused::Duration)
        );
    }

    let handles: Vec<_> = (0..4)
        .map(|_| {
            table
                .show(0, request(Light::Food, Pattern::Steady, 1, State::Low))
                .unwrap()
        })
        .collect();
    assert_eq!(
        table.show(0, request(Light::Lock, Pattern::Steady, 1, State::Low)),
        Err(Refused::Full)
    );
    assert!(table.cancel(handles[2]));
    assert!(
        table
            .show(0, request(Light::Lock, Pattern::Steady, 1, State::Low))
            .is_ok()
    );
}

#[test]
fn a_claim_keeps_an_unchanged_request_and_replaces_a_changed_one() {
    let mut table = Table::new();
    let mut claim = Claim::new();
    let blink = request(Light::Food, BLINK, 5, State::Mid);

    claim.set(&mut table, 0, Some(blink)).unwrap();
    // Held at 150 the blink keeps the phase it started at 0.
    claim.set(&mut table, 150, Some(blink)).unwrap();
    assert!(!lit(table.frame(150), Light::Food));

    let steady = request(Light::Food, Pattern::Steady, 5, State::Mid);
    claim.set(&mut table, 200, Some(steady)).unwrap();
    assert!(lit(table.frame(200), Light::Food));
    assert_eq!(
        table.winner(Light::Food).unwrap().pattern,
        Pattern::Steady,
        "the old request was cancelled, not stacked"
    );

    claim.set(&mut table, 300, None).unwrap();
    assert!(table.winner(Light::Food).is_none());

    // A claim whose request expired shows it again.
    let flash = Request {
        lasts_ms: Some(100),
        ..steady
    };
    claim.set(&mut table, 400, Some(flash)).unwrap();
    table.frame(500);
    assert!(table.winner(Light::Food).is_none());
    claim.set(&mut table, 500, Some(flash)).unwrap();
    assert!(lit(table.frame(500), Light::Food));
}

/// A board that records each write and check, answering checks with
/// `check`; `refuse` refuses every write.
#[derive(Default)]
struct Board {
    levels: [bool; Light::COUNT],
    writes: Vec<(Light, bool)>,
    checks: Vec<(Light, bool)>,
    check: Option<Check>,
    refuse: bool,
    all_off: u32,
}

impl Lights for Board {
    const SETTLE_MS: u32 = 10;

    fn set(&mut self, light: Light, on: bool) -> bool {
        if self.refuse {
            return false;
        }
        self.writes.push((light, on));
        self.levels[light.index()] = on;
        true
    }

    fn check(&mut self, light: Light, on: bool) -> Check {
        self.checks.push((light, on));
        self.check.unwrap_or(if self.levels[light.index()] == on {
            Check::Held
        } else {
            Check::Abort
        })
    }

    fn all_off(&mut self) {
        self.all_off += 1;
        self.levels = [false; Light::COUNT];
    }
}

fn levels(lit: &[Light]) -> [bool; Light::COUNT] {
    let mut levels = [false; Light::COUNT];
    for light in lit {
        levels[light.index()] = true;
    }
    levels
}

#[test]
fn the_driver_writes_a_change_once_and_checks_it_after_it_settles() {
    let mut board = Board::default();
    let mut drive = Drive::new();

    let step = drive.step(&mut board, 0, levels(&[]));
    assert_eq!(step.delay_ms, u32::MAX);
    assert!(board.writes.is_empty(), "every light starts off");

    let want = levels(&[Light::Lock, Light::Fault]);
    let step = drive.step(&mut board, 100, want);
    assert_eq!(step.delay_ms, 10);
    assert_eq!(board.writes, [(Light::Lock, true), (Light::Fault, true)]);

    let step = drive.step(&mut board, 104, want);
    assert_eq!(step.delay_ms, 6);
    assert!(board.checks.is_empty());

    let step = drive.step(&mut board, 110, want);
    assert_eq!(step.delay_ms, u32::MAX);
    assert_eq!(board.checks, [(Light::Lock, true), (Light::Fault, true)]);

    drive.step(&mut board, 200, want);
    assert_eq!(board.writes.len(), 2, "a held level is not written again");
    assert_eq!(board.checks.len(), 2);
}

#[test]
fn a_level_that_changes_while_settling_is_written_behind_its_check() {
    let mut board = Board::default();
    let mut drive = Drive::new();

    drive.step(&mut board, 0, levels(&[Light::Food]));
    drive.step(&mut board, 5, levels(&[]));
    assert_eq!(board.writes, [(Light::Food, true)]);

    let step = drive.step(&mut board, 10, levels(&[]));
    assert_eq!(board.checks, [(Light::Food, true)]);
    assert_eq!(board.writes, [(Light::Food, true), (Light::Food, false)]);
    assert_eq!(step.delay_ms, 10);
}

#[test]
fn a_failed_check_or_refused_write_turns_every_light_off_for_good() {
    for (check, refuse) in [
        (Some(Check::Abort), false),
        (Some(Check::InputMismatch), false),
        (None, true),
    ] {
        let mut board = Board {
            check,
            refuse,
            ..Board::default()
        };
        let mut drive = Drive::new();

        let mut step = drive.step(&mut board, 0, levels(&[Light::Connection]));
        if !refuse {
            step = drive.step(&mut board, 10, levels(&[Light::Connection]));
        }
        assert_eq!(
            step.failure,
            Some(Failure {
                light: Light::Connection,
                check
            })
        );
        assert!(drive.failed());
        assert_eq!(board.all_off, 1);

        // Nothing is driven again, and the failure is reported once.
        let writes = board.writes.len();
        let step = drive.step(&mut board, 20, levels(&[Light::Lock]));
        assert_eq!(step.failure, None);
        assert_eq!(step.delay_ms, u32::MAX);
        assert_eq!(board.writes.len(), writes);
        assert_eq!(board.levels, [false; Light::COUNT]);
    }
}

#[test]
fn a_blink_through_the_driver_writes_each_edge() {
    let mut table = Table::new();
    let mut board = Board::default();
    let mut drive = Drive::new();
    table
        .show(0, request(Light::Lock, BLINK, 1, State::Low))
        .unwrap();

    // Step as a loop would: at the sooner of the two delays.
    let mut now = 0;
    while now < 1_600 {
        let frame = table.frame(now);
        let step = drive.step(&mut board, now, frame.levels);
        assert_eq!(step.failure, None);
        now += frame.next_ms.min(step.delay_ms);
    }
    let edges: Vec<bool> = board.writes.iter().map(|&(_, on)| on).collect();
    assert_eq!(edges, [true, false, true, false, true, false, true, false]);
    assert_eq!(board.checks.len(), 8);
}

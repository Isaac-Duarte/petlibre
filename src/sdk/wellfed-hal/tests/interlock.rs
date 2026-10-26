//! Feeds and Wi-Fi joins take turns: no feed starts during a join, a join
//! waits while outputs are claimed, and neither waits forever.

use std::cell::Cell;

use wellfed_hal::interlock::{Held, Interlock, JOIN_POLL_MS, JoinStart, Turn};

/// The longest one feed may hold the outputs, for these tests.
const BOUND_MS: u32 = 10_000;

/// A fake clock that the join's waits advance, and a hook run at each wait.
struct Clock {
    now: Cell<u32>,
    waits: Cell<u32>,
}

impl Clock {
    fn new() -> Self {
        Self {
            now: Cell::new(0),
            waits: Cell::new(0),
        }
    }

    /// Joins on `interlock`, running `at_wait` with the wait's number (from 1)
    /// at each wait.
    fn join(&self, interlock: &Interlock, at_wait: impl Fn(u32)) -> JoinStart {
        interlock.begin_join(
            || self.now.get(),
            |ms| {
                self.now.set(self.now.get() + ms);
                self.waits.set(self.waits.get() + 1);
                at_wait(self.waits.get());
            },
        )
    }
}

#[test]
fn a_feed_during_a_join_is_held() {
    let interlock = Interlock::new();
    let clock = Clock::new();

    assert_eq!(clock.join(&interlock, |_| {}), JoinStart::Free);
    assert_eq!(interlock.turn(), Turn::Joining);
    assert_eq!(interlock.claim_feed(BOUND_MS), Err(Held::Joining));
    assert_eq!(interlock.status().refused, 1);

    interlock.end_join();
    assert_eq!(interlock.turn(), Turn::Free);
    assert_eq!(interlock.claim_feed(BOUND_MS), Ok(()));
    assert_eq!(interlock.turn(), Turn::Feeding);
}

#[test]
fn a_join_during_a_feed_waits_until_the_outputs_are_released() {
    let interlock = Interlock::new();
    let clock = Clock::new();
    interlock.claim_feed(BOUND_MS).unwrap();

    // The loop sees the owner at rest at the third wait.
    let start = clock.join(&interlock, |wait| {
        assert_ne!(interlock.turn(), Turn::Joining, "no join while claimed");
        if wait == 3 {
            interlock.release_feed();
        }
    });

    assert_eq!(
        start,
        JoinStart::Waited {
            ms: 3 * JOIN_POLL_MS
        }
    );
    assert_eq!(interlock.turn(), Turn::Joining);
    let status = interlock.status();
    assert_eq!((status.deferred, status.forced), (1, 0));
    assert_eq!(status.waited_max_ms, 3 * JOIN_POLL_MS);
}

#[test]
fn a_waiting_join_goes_before_the_next_feed() {
    let interlock = Interlock::new();
    let clock = Clock::new();
    interlock.claim_feed(BOUND_MS).unwrap();

    let start = clock.join(&interlock, |wait| match wait {
        // A second feed while the join waits is refused, so the join cannot
        // be starved by feeds back to back.
        1 => assert_eq!(interlock.claim_feed(BOUND_MS), Err(Held::JoinWaiting)),
        // The release hands the turn to the join, not to a new feed.
        2 => {
            interlock.release_feed();
            assert_eq!(interlock.turn(), Turn::Joining);
            assert_eq!(interlock.claim_feed(BOUND_MS), Err(Held::Joining));
        }
        _ => {}
    });

    assert_eq!(
        start,
        JoinStart::Waited {
            ms: 2 * JOIN_POLL_MS
        }
    );
    interlock.end_join();
    assert_eq!(interlock.claim_feed(BOUND_MS), Ok(()));
}

#[test]
fn a_claim_never_released_holds_a_join_only_for_its_bound() {
    let interlock = Interlock::new();
    let clock = Clock::new();
    interlock.claim_feed(BOUND_MS).unwrap();

    let start = clock.join(&interlock, |_| {});

    assert_eq!(start, JoinStart::Forced { ms: BOUND_MS });
    assert_eq!(clock.waits.get(), BOUND_MS / JOIN_POLL_MS);
    assert_eq!(interlock.turn(), Turn::Joining);
    assert_eq!(interlock.status().forced, 1);

    // The stale claim's release does not end the join; the join's end
    // frees the turn for the next feed.
    interlock.release_feed();
    assert_eq!(interlock.turn(), Turn::Joining);
    interlock.end_join();
    assert_eq!(interlock.claim_feed(BOUND_MS), Ok(()));
}

#[test]
fn releasing_with_nothing_claimed_changes_nothing() {
    let interlock = Interlock::new();

    interlock.release_feed();
    assert_eq!(interlock.turn(), Turn::Free);

    let clock = Clock::new();
    clock.join(&interlock, |_| {});
    interlock.release_feed();
    assert_eq!(interlock.turn(), Turn::Joining);
}

#[test]
fn every_turn_round_trips_its_byte() {
    for turn in [
        Turn::Free,
        Turn::Joining,
        Turn::Feeding,
        Turn::FeedingJoinWaits,
    ] {
        assert_eq!(Turn::from_byte(turn.code()), turn);
    }
    // An unknown byte reads as the outputs claimed: no join goes early.
    assert_eq!(Turn::from_byte(0xff), Turn::Feeding);
}

/// Feeds and joins from two threads, as the loop and the Wi-Fi task: a
/// join's turn never overlaps a claim, and both sides make progress.
#[test]
fn two_tasks_never_overlap_and_neither_starves() {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::thread;

    static INTERLOCK: Interlock = Interlock::new();
    static JOINING: AtomicBool = AtomicBool::new(false);
    static FEEDING: AtomicBool = AtomicBool::new(false);
    static OVERLAPS: AtomicU32 = AtomicU32::new(0);
    const ROUNDS: u32 = 2000;

    let joins = thread::spawn(|| {
        let mut joined = 0;
        for _ in 0..ROUNDS {
            let start = INTERLOCK.begin_join(|| 0, |_| thread::yield_now());
            assert!(!matches!(start, JoinStart::Forced { .. }));
            JOINING.store(true, Ordering::SeqCst);
            if FEEDING.load(Ordering::SeqCst) {
                OVERLAPS.fetch_add(1, Ordering::SeqCst);
            }
            JOINING.store(false, Ordering::SeqCst);
            INTERLOCK.end_join();
            joined += 1;
        }
        joined
    });

    let mut fed = 0;
    while !joins.is_finished() || fed == 0 {
        if INTERLOCK.claim_feed(u32::MAX).is_ok() {
            FEEDING.store(true, Ordering::SeqCst);
            if JOINING.load(Ordering::SeqCst) {
                OVERLAPS.fetch_add(1, Ordering::SeqCst);
            }
            FEEDING.store(false, Ordering::SeqCst);
            INTERLOCK.release_feed();
            fed += 1;
        }
        thread::yield_now();
    }

    assert_eq!(joins.join().unwrap(), ROUNDS);
    assert!(fed > 0);
    assert_eq!(OVERLAPS.load(Ordering::SeqCst), 0);
}

#[test]
fn a_join_holding_its_turn_too_long_is_counted_once() {
    use wellfed_hal::interlock::LONG_JOIN_MS;

    let interlock = Interlock::new();
    let clock = Clock::new();
    clock.now.set(1000);
    clock.join(&interlock, |_| {});

    assert!(!interlock.watch_join(1000 + LONG_JOIN_MS - 1));
    assert!(interlock.watch_join(1000 + LONG_JOIN_MS));
    assert!(
        !interlock.watch_join(1000 + 2 * LONG_JOIN_MS),
        "once per join"
    );
    assert_eq!(interlock.status().long_joins, 1);

    // The next join is watched afresh, from its own turn.
    interlock.end_join();
    assert!(!interlock.watch_join(1000 + 3 * LONG_JOIN_MS));
    clock.now.set(500_000);
    clock.join(&interlock, |_| {});
    assert!(!interlock.watch_join(500_000 + 1));
    assert!(interlock.watch_join(500_000 + LONG_JOIN_MS));
    assert_eq!(interlock.status().long_joins, 2);
}

#[test]
fn a_waiting_join_is_watched_from_when_it_gets_the_turn() {
    use wellfed_hal::interlock::LONG_JOIN_MS;

    let interlock = Interlock::new();
    let clock = Clock::new();
    interlock.claim_feed(u32::MAX).unwrap();

    // Waits LONG_JOIN_MS for the feed, then gets the turn.
    let waits = LONG_JOIN_MS / JOIN_POLL_MS;
    clock.join(&interlock, |wait| {
        assert!(
            !interlock.watch_join(clock.now.get()),
            "waiting, not joining"
        );
        if wait == waits {
            interlock.release_feed();
        }
    });
    assert!(!interlock.watch_join(LONG_JOIN_MS + 1));
    assert!(interlock.watch_join(2 * LONG_JOIN_MS));
}

mod writes {
    //! Storage writes and feeds take turns: no write starts while the
    //! outputs are claimed, no feed starts while a write runs, and neither
    //! waits forever.

    use std::cell::Cell;

    use wellfed_hal::interlock::{
        Held, Interlock, JoinStart, Turn, WRITE_POLL_MS, WRITE_WAIT_MS, WriteStart, WriteStep,
        WriteTurn,
    };

    use super::{BOUND_MS, Clock};

    /// Claims on `interlock` after writes, with `at_wait` at each wait.
    fn claim(
        interlock: &Interlock,
        now: &Cell<u32>,
        mut at_wait: impl FnMut(u32),
    ) -> Result<(), Held> {
        interlock.claim_feed_after_writes(
            BOUND_MS,
            || false,
            || now.get(),
            |ms| {
                now.set(now.get() + ms);
                at_wait(now.get());
            },
        )
    }

    #[test]
    fn a_write_during_a_feed_waits_and_goes_before_the_next_feed() {
        let interlock = Interlock::new();
        let mut turn = WriteTurn::new();
        interlock.claim_feed(BOUND_MS).unwrap();

        assert_eq!(turn.begin(&interlock, 100, || true), WriteStep::Wait);
        assert_eq!(turn.begin(&interlock, 300, || true), WriteStep::Wait);
        assert!(!interlock.writing());

        // The release leaves the write marked waiting, so the next claim
        // waits for it; the write's next poll takes the turn.
        interlock.release_feed();
        let now = Cell::new(1000);
        let mut taken = None;
        let result = claim(&interlock, &now, |at| {
            if at == 1000 + 2 * WRITE_POLL_MS {
                taken = Some(turn.begin(&interlock, at, || true));
                assert_eq!(interlock.claim_feed(BOUND_MS), Err(Held::Writing));
            }
            if at == 1000 + 5 * WRITE_POLL_MS {
                interlock.end_write();
            }
        });

        assert_eq!(result, Ok(()));
        assert_eq!(
            taken,
            Some(WriteStep::Go(WriteStart::Waited {
                ms: 1000 + 2 * WRITE_POLL_MS - 100
            }))
        );
        assert_eq!(interlock.turn(), Turn::Feeding);
        let status = interlock.status();
        assert_eq!((status.writes_deferred, status.writes_forced), (1, 0));
        assert_eq!(status.feeds_delayed, 1);
    }

    #[test]
    fn a_write_still_running_past_the_wait_refuses_the_feed() {
        let interlock = Interlock::new();
        let mut turn = WriteTurn::new();
        assert_eq!(
            turn.begin(&interlock, 0, || true),
            WriteStep::Go(WriteStart::Free)
        );

        let now = Cell::new(0);
        let waits = Cell::new(0);
        let result = claim(&interlock, &now, |_| waits.set(waits.get() + 1));

        assert_eq!(result, Err(Held::Writing));
        assert_eq!(now.get(), WRITE_WAIT_MS);
        assert_eq!(waits.get(), WRITE_WAIT_MS / WRITE_POLL_MS);
        assert_eq!(interlock.turn(), Turn::Free);
        assert_eq!(interlock.status().refused, 1);

        interlock.end_write();
        assert_eq!(claim(&interlock, &now, |_| {}), Ok(()));
    }

    #[test]
    fn a_waiting_write_that_never_asks_again_is_passed() {
        let interlock = Interlock::new();
        let mut turn = WriteTurn::new();
        interlock.claim_feed(BOUND_MS).unwrap();
        assert_eq!(turn.begin(&interlock, 0, || true), WriteStep::Wait);
        interlock.release_feed();

        let now = Cell::new(0);
        assert_eq!(claim(&interlock, &now, |_| {}), Ok(()));
        assert_eq!(now.get(), WRITE_WAIT_MS);

        // It finds the new claim when it does ask, and waits again.
        assert_eq!(
            turn.begin(&interlock, WRITE_WAIT_MS, || true),
            WriteStep::Wait
        );
        interlock.release_feed();
        assert!(matches!(
            turn.begin(&interlock, WRITE_WAIT_MS + 1, || true),
            WriteStep::Go(WriteStart::Waited { .. })
        ));
    }

    #[test]
    fn a_claim_never_released_holds_a_write_only_for_its_bound() {
        let interlock = Interlock::new();
        let mut turn = WriteTurn::new();
        interlock.claim_feed(BOUND_MS).unwrap();

        assert_eq!(turn.begin(&interlock, 0, || true), WriteStep::Wait);
        assert_eq!(
            turn.begin(&interlock, BOUND_MS - 1, || true),
            WriteStep::Wait
        );
        assert_eq!(
            turn.begin(&interlock, BOUND_MS, || true),
            WriteStep::Go(WriteStart::Forced { ms: BOUND_MS })
        );
        assert!(interlock.writing());
        assert_eq!(interlock.status().writes_forced, 1);

        // The claim's release and the write's end each free their own part.
        interlock.release_feed();
        assert!(interlock.writing());
        interlock.end_write();
        assert_eq!(interlock.claim_feed(BOUND_MS), Ok(()));
    }

    #[test]
    fn one_write_at_a_time() {
        let interlock = Interlock::new();
        let (mut first, mut second) = (WriteTurn::new(), WriteTurn::new());

        assert_eq!(
            first.begin(&interlock, 0, || true),
            WriteStep::Go(WriteStart::Free)
        );
        assert_eq!(second.begin(&interlock, 0, || true), WriteStep::Wait);
        interlock.end_write();
        assert!(matches!(
            second.begin(&interlock, 10, || true),
            WriteStep::Go(WriteStart::Waited { ms: 10 })
        ));
    }

    /// A store write sets the attempt marker, and a reset that lands in one
    /// makes the next boot skip arming: none starts while the watchdog
    /// feeder withholds the watchdog, transiently or for good.
    #[test]
    fn no_write_starts_while_the_watchdog_is_withheld() {
        use wellfed_hal::supervision::{self, config::WATCHDOG};

        let interlock = Interlock::new();
        let mut turn = WriteTurn::new();
        supervision::reset();

        supervision::supervised(false, 1000);
        assert_eq!(turn.begin(&interlock, 1000, || true), WriteStep::Wait);
        // Feeding again before the progress gap: the write goes.
        supervision::supervised(true, 1500);
        assert_eq!(
            turn.begin(&interlock, 1500, || true),
            WriteStep::Go(WriteStart::Free)
        );
        interlock.end_write();

        // A stop that outlasts the gap is a certain reset: no write again,
        // even once `feed` would say yes.
        supervision::supervised(false, 2000);
        supervision::supervised(false, 2000 + WATCHDOG.max_progress_gap_ms);
        supervision::supervised(true, 9000);
        assert_eq!(turn.begin(&interlock, 9000, || true), WriteStep::Wait);
        assert!(!interlock.writing());
        supervision::reset();
    }

    #[test]
    fn writes_and_joins_do_not_wait_for_each_other() {
        let interlock = Interlock::new();
        let mut turn = WriteTurn::new();
        let clock = Clock::new();

        assert_eq!(clock.join(&interlock, |_| {}), JoinStart::Free);
        assert_eq!(
            turn.begin(&interlock, 0, || true),
            WriteStep::Go(WriteStart::Free)
        );
        interlock.end_join();
        assert_eq!(clock.join(&interlock, |_| {}), JoinStart::Free);
        interlock.end_join();
        interlock.end_write();

        // A join still refuses a feed at once, without the write wait.
        clock.join(&interlock, |_| {});
        assert_eq!(
            turn.begin(&interlock, 0, || true),
            WriteStep::Go(WriteStart::Free)
        );
        let now = Cell::new(0);
        assert_eq!(claim(&interlock, &now, |_| {}), Err(Held::Joining));
        assert_eq!(now.get(), 0);
    }

    #[test]
    fn a_writer_that_misses_the_gap_is_timed_again_on_the_next_feed() {
        let interlock = Interlock::new();
        let mut turn = WriteTurn::new();
        interlock.claim_feed(BOUND_MS).unwrap();
        assert_eq!(turn.begin(&interlock, 0, || true), WriteStep::Wait);
        assert_eq!(
            turn.begin(&interlock, BOUND_MS - 10, || true),
            WriteStep::Wait
        );

        // Feed A ends and feed B is claimed before the writer asks again
        // (B passed the writer after its wait).
        interlock.release_feed();
        let now = Cell::new(BOUND_MS - 5);
        let claimed = interlock.claim_feed_after_writes(
            BOUND_MS,
            || false,
            || now.get(),
            |ms| now.set(now.get() + ms),
        );
        assert_eq!(claimed, Ok(()));

        // Past A's bound counted from the writer's first wait, but B's own
        // claim is young: the writer waits, timed from when it saw B.
        let at = BOUND_MS + WRITE_WAIT_MS;
        assert_eq!(turn.begin(&interlock, at, || true), WriteStep::Wait);
        assert_eq!(
            turn.begin(&interlock, at + BOUND_MS - 1, || true),
            WriteStep::Wait
        );
        assert!(matches!(
            turn.begin(&interlock, at + BOUND_MS, || true),
            WriteStep::Go(WriteStart::Forced { ms: BOUND_MS })
        ));
    }

    #[test]
    fn a_write_is_never_forced_into_moving_outputs() {
        let interlock = Interlock::new();
        let mut turn = WriteTurn::new();
        interlock.claim_feed(BOUND_MS).unwrap();

        assert_eq!(turn.begin(&interlock, 0, || false), WriteStep::Wait);
        assert_eq!(
            turn.begin(&interlock, 10 * BOUND_MS, || false),
            WriteStep::Wait
        );
        assert!(!interlock.writing());
        assert_eq!(interlock.status().writes_forced, 0);
        assert!(matches!(
            turn.begin(&interlock, 10 * BOUND_MS + 1, || true),
            WriteStep::Go(WriteStart::Forced { .. })
        ));
    }

    #[test]
    fn work_queued_for_a_writer_delays_the_claim_until_it_is_written() {
        let interlock = Interlock::new();
        let queued = Cell::new(true);
        let now = Cell::new(0);

        // The writer takes the queued change, writes it and finishes it.
        let claimed = interlock.claim_feed_after_writes(
            BOUND_MS,
            || queued.get(),
            || now.get(),
            |ms| {
                now.set(now.get() + ms);
                if now.get() == 200 {
                    queued.set(false);
                }
            },
        );
        assert_eq!(claimed, Ok(()));
        assert_eq!(now.get(), 200);
        interlock.release_feed();

        // Work that never moves (a stalled writer) refuses the feed at the
        // deadline: nothing moved, and the change would land after it.
        now.set(0);
        let claimed = interlock.claim_feed_after_writes(
            BOUND_MS,
            || true,
            || now.get(),
            |ms| now.set(now.get() + ms),
        );
        assert_eq!(claimed, Err(Held::Writing));
        assert_eq!(now.get(), WRITE_WAIT_MS);
        assert_eq!(interlock.turn(), Turn::Free);
    }

    #[test]
    fn a_claim_holds_handshakes_only_for_its_bound() {
        let interlock = Interlock::new();
        assert!(!interlock.claim_in_bound(0));

        let claimed =
            interlock.claim_feed_after_writes(BOUND_MS, || false, || 1000, |_| unreachable!());
        assert_eq!(claimed, Ok(()));
        assert!(interlock.claim_in_bound(1000));
        assert!(interlock.claim_in_bound(1000 + BOUND_MS - 1));
        // A claim never released stops holding them, as it stops holding joins.
        assert!(!interlock.claim_in_bound(1000 + BOUND_MS));

        interlock.release_feed();
        assert!(!interlock.claim_in_bound(1001));
    }

    #[test]
    fn one_writers_turn_keeps_anothers_wait_marked() {
        let interlock = Interlock::new();
        let (mut first, mut second) = (WriteTurn::new(), WriteTurn::new());
        interlock.claim_feed(BOUND_MS).unwrap();
        assert_eq!(first.begin(&interlock, 0, || true), WriteStep::Wait);
        assert_eq!(second.begin(&interlock, 0, || true), WriteStep::Wait);
        interlock.release_feed();

        assert!(matches!(
            first.begin(&interlock, 5, || true),
            WriteStep::Go(_)
        ));
        interlock.end_write();
        // The second still waits: a new claim lets it go first.
        assert_eq!(interlock.claim_feed(BOUND_MS), Err(Held::WriteWaiting));
        assert!(matches!(
            second.begin(&interlock, 6, || true),
            WriteStep::Go(_)
        ));
        interlock.end_write();
        assert_eq!(interlock.claim_feed(BOUND_MS), Ok(()));
    }

    /// Writes and feeds from two threads, as the service task and the loop:
    /// a write never overlaps a claim, and both sides make progress.
    #[test]
    fn two_tasks_never_overlap_and_neither_starves() {
        use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
        use std::thread;

        static INTERLOCK: Interlock = Interlock::new();
        static WRITING: AtomicBool = AtomicBool::new(false);
        static FEEDING: AtomicBool = AtomicBool::new(false);
        static OVERLAPS: AtomicU32 = AtomicU32::new(0);
        const ROUNDS: u32 = 2000;

        let writes = thread::spawn(|| {
            let mut turn = WriteTurn::new();
            let mut written = 0;
            while written < ROUNDS {
                let WriteStep::Go(start) = turn.begin(&INTERLOCK, 0, || true) else {
                    thread::yield_now();
                    continue;
                };
                assert!(!matches!(start, WriteStart::Forced { .. }));
                WRITING.store(true, Ordering::SeqCst);
                if FEEDING.load(Ordering::SeqCst) {
                    OVERLAPS.fetch_add(1, Ordering::SeqCst);
                }
                WRITING.store(false, Ordering::SeqCst);
                INTERLOCK.end_write();
                written += 1;
            }
            written
        });

        let mut fed = 0;
        while !writes.is_finished() || fed == 0 {
            let claimed = INTERLOCK.claim_feed_after_writes(
                u32::MAX,
                || false,
                || 0,
                |_| thread::yield_now(),
            );
            if claimed.is_ok() {
                FEEDING.store(true, Ordering::SeqCst);
                if WRITING.load(Ordering::SeqCst) {
                    OVERLAPS.fetch_add(1, Ordering::SeqCst);
                }
                FEEDING.store(false, Ordering::SeqCst);
                INTERLOCK.release_feed();
                fed += 1;
            }
            thread::yield_now();
        }

        assert_eq!(writes.join().unwrap(), ROUNDS);
        assert!(fed > 0);
        assert_eq!(OVERLAPS.load(Ordering::SeqCst), 0);
    }
}

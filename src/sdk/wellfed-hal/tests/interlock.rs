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

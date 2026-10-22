//! Tests for portion counting and direction alternation within one feed
//! request.
//!
//! The double-bowl trace is an independent reconstruction of the stock
//! `Micro_SwHandle` behavior. A finished request never counts further, even
//! at the `u16` maximum.

use petlibre_core::feed_progress::{BowlMode, Direction, FeedProgress, ProgressResult};

#[test]
fn single_bowl_request() {
    let mut progress = FeedProgress::new(BowlMode::SINGLE, Direction::D0, 3, true).unwrap();
    assert_eq!(progress.accept(), ProgressResult::PORTION);
    assert_eq!(progress.completed, 1);
    assert_eq!(progress.accept(), ProgressResult::PORTION);
    assert_eq!(progress.completed, 2);
    assert_eq!(progress.accept(), ProgressResult::FINISHED);
    assert_eq!(progress.completed, 3);
    assert_eq!(progress.direction, Direction::D0);

    for _ in 0..100 {
        assert_eq!(progress.accept(), ProgressResult::FINISHED);
    }
    assert_eq!(progress.completed, 3);
}

#[test]
fn double_bowl_trace() {
    // Independent trace of stock Micro_SwHandle: three portions, five events.
    let completed = [1, 1, 2, 2, 3];
    let direction = [
        Direction::D1,
        Direction::D1,
        Direction::D0,
        Direction::D0,
        Direction::D0,
    ];
    let results = [
        ProgressResult::PORTION,
        ProgressResult::WAIT,
        ProgressResult::PORTION,
        ProgressResult::WAIT,
        ProgressResult::FINISHED,
    ];

    let mut progress = FeedProgress::new(BowlMode::DOUBLE, Direction::D0, 3, true).unwrap();
    for i in 0..completed.len() {
        assert_eq!(progress.accept(), results[i]);
        assert_eq!(progress.completed, completed[i]);
        assert_eq!(progress.direction, direction[i]);
    }

    assert!(progress.credit_next_event);
    assert_eq!(progress.accept(), ProgressResult::FINISHED);
    assert_eq!(progress.completed, 3);
    assert_eq!(progress.direction, Direction::D0);
}

#[test]
fn double_bowl_without_initial_credit() {
    let mut progress = FeedProgress::new(BowlMode::DOUBLE, Direction::D1, 1, false).unwrap();
    assert_eq!(progress.accept(), ProgressResult::WAIT);
    assert_eq!(progress.completed, 0);
    assert_eq!(progress.accept(), ProgressResult::FINISHED);
    assert_eq!(progress.completed, 1);
    assert_eq!(progress.direction, Direction::D1);
}

#[test]
fn consecutive_requests() {
    let mut progress = FeedProgress::new(BowlMode::DOUBLE, Direction::D1, 2, true).unwrap();
    assert_eq!(progress.accept(), ProgressResult::PORTION);
    assert_eq!(progress.accept(), ProgressResult::WAIT);
    assert_eq!(progress.accept(), ProgressResult::FINISHED);
    assert_eq!(progress.direction, Direction::D0);

    progress = FeedProgress::new(
        progress.mode,
        progress.direction,
        1,
        progress.credit_next_event,
    )
    .unwrap();

    assert_eq!(progress.accept(), ProgressResult::FINISHED);
    assert_eq!(progress.completed, 1);
    assert_eq!(progress.direction, Direction::D0);
}

#[test]
fn maximum_request_does_not_wrap() {
    let mut progress = FeedProgress::new(BowlMode::DOUBLE, Direction::D0, u16::MAX, true).unwrap();
    let mut events = 0u32;

    while !progress.finished {
        progress.accept();
        events += 1;
        assert!(events <= 131_069);
    }

    assert_eq!(events, 131_069);
    assert_eq!(progress.completed, u16::MAX);
    assert_eq!(progress.accept(), ProgressResult::FINISHED);
    assert_eq!(progress.completed, u16::MAX);
}

#[test]
fn invalid_requests_are_rejected() {
    assert!(FeedProgress::new(BowlMode::SINGLE, Direction::D0, 0, true).is_none());
    assert!(FeedProgress::new(BowlMode(2), Direction::D0, 1, true).is_none());
    assert!(FeedProgress::new(BowlMode::SINGLE, Direction(2), 1, true).is_none());
}

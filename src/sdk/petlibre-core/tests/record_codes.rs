//! The bytes the feed journal and the settings record store for each enum
//! variant. Flash written by an earlier build must keep decoding, so these
//! numbers never change without a record migration.

use petlibre_core::feed::controller::FeedReason;
use petlibre_core::feed::progress::{BowlMode, Direction};
use petlibre_core::feed::storage::OwnerOutcome;

#[test]
fn persisted_variants_keep_their_bytes() {
    let modes = [(BowlMode::Single, 0), (BowlMode::Double, 1)];
    for (mode, byte) in modes {
        assert_eq!(mode.byte(), byte);
        assert_eq!(BowlMode::from_byte(byte), Some(mode));
    }

    let directions = [(Direction::D0, 0), (Direction::D1, 1)];
    for (direction, byte) in directions {
        assert_eq!(direction.byte(), byte);
        assert_eq!(Direction::from_byte(byte), Some(direction));
    }

    let outcomes = [
        (OwnerOutcome::Pending, 0),
        (OwnerOutcome::Done, 1),
        (OwnerOutcome::Cancelled, 2),
        (OwnerOutcome::Expired, 3),
        (OwnerOutcome::Failed, 4),
    ];
    for (outcome, byte) in outcomes {
        assert_eq!(outcome.byte(), byte);
        assert_eq!(OwnerOutcome::from_byte(byte), Some(outcome));
    }

    let reasons = [
        (FeedReason::None, 0),
        (FeedReason::Done, 1),
        (FeedReason::Cancelled, 2),
        (FeedReason::Clock, 3),
        (FeedReason::ObservationGap, 4),
        (FeedReason::TotalTimeout, 5),
        (FeedReason::FeedbackTimeout, 6),
        (FeedReason::Power, 7),
        (FeedReason::Sensor, 8),
        (FeedReason::Outlet, 9),
        (FeedReason::Overcurrent, 10),
        (FeedReason::FeedbackStream, 11),
        (FeedReason::Output, 12),
    ];
    for (reason, byte) in reasons {
        assert_eq!(reason.byte(), byte);
        assert_eq!(FeedReason::from_byte(byte), Some(reason));
    }
}

#[test]
fn unknown_bytes_name_no_variant() {
    assert_eq!(BowlMode::from_byte(2), None);
    assert_eq!(Direction::from_byte(2), None);
    assert_eq!(OwnerOutcome::from_byte(5), None);
    assert_eq!(FeedReason::from_byte(13), None);
    assert_eq!(FeedReason::from_byte(u8::MAX), None);
}

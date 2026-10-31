//! A feed's end as the history records it, from the feeding owner's result.
//!
//! A feed runs whatever the food beam reads, as stock's does, so a beam
//! that misreads never skips a meal; the event records what it read
//! (`hopper`), low food included.
//!
//! The owner's result is durable once it reports it; one that is not (the
//! owner faulted before its journal write) may have dispensed anything up
//! to what was asked, so it is recorded as uncertain and never retried. A
//! feed a reset interrupted is resolved at the next boot, durably, as
//! uncertain too, with a reason of its own ([`interrupted`]).
//!
//! The journal keeps no reason for a resolved feed (its record is pinned,
//! and bank 0 and the flash tools read it), so the reason is the wire's
//! alone: the boot that resolves the feed names it.

use wellfed_core::feed::controller::FeedReason as CoreReason;
use wellfed_core::feed::storage::{OwnerOutcome, OwnerResult};
use wellfed_proto::api::{EventBody, FeedOutcome, FeedReason, FoodLevel, Source};

/// Whether the journal holds the result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Durability {
    Durable,
    Lost,
}

/// What the food beam read during a feed, as the owner's trace keeps it
/// (`Some(false)` once it read empty).
pub const fn food_level(reading: Option<bool>) -> FoodLevel {
    match reading {
        None => FoodLevel::Unread,
        Some(true) => FoodLevel::Present,
        Some(false) => FoodLevel::Low,
    }
}

/// The `Fed` event for the feed `feed`, which `result` ended, with the food
/// beam reading `hopper` during it.
pub fn fed(
    feed: u32,
    result: &OwnerResult,
    hopper: Option<bool>,
    durability: Durability,
    by: Source,
) -> EventBody {
    let outcome = match (durability, result.outcome) {
        (Durability::Lost, _) | (_, OwnerOutcome::Pending | OwnerOutcome::Uncertain) => {
            FeedOutcome::Uncertain
        }
        (Durability::Durable, OwnerOutcome::Done) => FeedOutcome::Done,
        (Durability::Durable, OwnerOutcome::Cancelled) => FeedOutcome::Cancelled,
        (Durability::Durable, OwnerOutcome::Expired) => FeedOutcome::Expired,
        (Durability::Durable, OwnerOutcome::Failed) => FeedOutcome::Failed,
    };
    EventBody::Fed {
        feed,
        requested: portions(result.requested),
        completed: portions(result.completed),
        outcome,
        reason: reason(result.reason),
        hopper: food_level(hopper),
        by,
    }
}

/// A count of portions as the event carries it; the owner caps a feed far
/// below 255.
fn portions(count: u16) -> u8 {
    u8::try_from(count).unwrap_or(u8::MAX)
}

/// The `Fed` event for the feed `feed` a reset interrupted, which the
/// owner resolved at this boot as `result`: uncertain, for the reason
/// `Interrupted`.
pub fn interrupted(feed: u32, result: &OwnerResult, by: Source) -> EventBody {
    EventBody::Fed {
        feed,
        requested: portions(result.requested),
        completed: portions(result.completed),
        outcome: FeedOutcome::Uncertain,
        reason: FeedReason::Interrupted,
        // The boot that resolves it never saw the feed's reading.
        hopper: FoodLevel::Unread,
        by,
    }
}

const fn reason(reason: CoreReason) -> FeedReason {
    match reason {
        CoreReason::Done => FeedReason::Done,
        CoreReason::Cancelled => FeedReason::Cancelled,
        CoreReason::Clock => FeedReason::Clock,
        CoreReason::ObservationGap => FeedReason::ObservationGap,
        CoreReason::TotalTimeout => FeedReason::Timeout,
        CoreReason::FeedbackTimeout => FeedReason::NoFeedback,
        CoreReason::Power => FeedReason::Power,
        CoreReason::Sensor => FeedReason::Sensor,
        CoreReason::Outlet => FeedReason::OutletBlocked,
        CoreReason::Jammed => FeedReason::Jammed,
        CoreReason::MarkStuck => FeedReason::MarkStuck,
        CoreReason::Overcurrent => FeedReason::Overcurrent,
        CoreReason::FeedbackStream => FeedReason::FeedbackStream,
        // No reason: a result still pending, which ends as an output
        // failure if it ends at all. A feed resolved at boot has none
        // either, but takes `interrupted` instead.
        CoreReason::None | CoreReason::Output => FeedReason::Output,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(outcome: OwnerOutcome, reason: CoreReason) -> OwnerResult {
        OwnerResult {
            id: 7,
            requested: 2,
            completed: 1,
            outcome,
            reason,
            motion_possible: true,
        }
    }

    #[test]
    fn a_durable_result_keeps_its_outcome_and_a_lost_one_is_uncertain() {
        let failed = result(OwnerOutcome::Failed, CoreReason::FeedbackTimeout);
        assert_eq!(
            fed(7, &failed, Some(false), Durability::Durable, Source::Panel),
            EventBody::Fed {
                feed: 7,
                requested: 2,
                completed: 1,
                outcome: FeedOutcome::Failed,
                reason: FeedReason::NoFeedback,
                hopper: FoodLevel::Low,
                by: Source::Panel,
            }
        );

        let done = result(OwnerOutcome::Done, CoreReason::Done);
        let EventBody::Fed { outcome, .. } =
            fed(7, &done, Some(true), Durability::Lost, Source::Feeder)
        else {
            panic!("not a feed");
        };
        assert_eq!(outcome, FeedOutcome::Uncertain);

        let pending = result(OwnerOutcome::Pending, CoreReason::None);
        let EventBody::Fed {
            outcome, reason, ..
        } = fed(7, &pending, None, Durability::Durable, Source::Feeder)
        else {
            panic!("not a feed");
        };
        assert_eq!(
            (outcome, reason),
            (FeedOutcome::Uncertain, FeedReason::Output)
        );
    }

    /// A feed runs on low food, as stock's does, and its event says the
    /// hopper read low; one reading of food and none low is food.
    #[test]
    fn the_event_records_what_the_food_beam_read() {
        let done = result(OwnerOutcome::Done, CoreReason::Done);
        for (reading, level) in [
            (Some(false), FoodLevel::Low),
            (Some(true), FoodLevel::Present),
            (None, FoodLevel::Unread),
        ] {
            let EventBody::Fed {
                outcome, hopper, ..
            } = fed(7, &done, reading, Durability::Durable, Source::Panel)
            else {
                panic!("not a feed");
            };
            assert_eq!((outcome, hopper), (FeedOutcome::Done, level));
        }
    }

    #[test]
    fn a_feed_resolved_at_boot_is_uncertain_interrupted_and_counts_no_portion() {
        let resolved = OwnerResult {
            completed: 0,
            motion_possible: true,
            ..result(OwnerOutcome::Uncertain, CoreReason::None)
        };
        assert_eq!(
            interrupted(9, &resolved, Source::Feeder),
            EventBody::Fed {
                feed: 9,
                requested: 2,
                completed: 0,
                outcome: FeedOutcome::Uncertain,
                reason: FeedReason::Interrupted,
                hopper: FoodLevel::Unread,
                by: Source::Feeder,
            }
        );
    }
}

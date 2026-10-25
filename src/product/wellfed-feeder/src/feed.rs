//! A feed's end as the history records it, from the feeding owner's result.
//!
//! The owner's result is durable once it reports it; one that is not (the
//! owner faulted before its journal write) may have dispensed anything up
//! to what was asked, so it is recorded as uncertain and never retried.

use wellfed_core::feed::controller::FeedReason as CoreReason;
use wellfed_core::feed::storage::{OwnerOutcome, OwnerResult};
use wellfed_proto::api::{EventBody, FeedOutcome, FeedReason, Source};

/// Whether the journal holds the result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Durability {
    Durable,
    Lost,
}

/// The `Fed` event for the feed `feed`, which `result` ended.
pub fn fed(feed: u32, result: &OwnerResult, durability: Durability, by: Source) -> EventBody {
    let outcome = match (durability, result.outcome) {
        (Durability::Lost, _) | (_, OwnerOutcome::Pending) => FeedOutcome::Uncertain,
        (Durability::Durable, OwnerOutcome::Done) => FeedOutcome::Done,
        (Durability::Durable, OwnerOutcome::Cancelled) => FeedOutcome::Cancelled,
        (Durability::Durable, OwnerOutcome::Expired) => FeedOutcome::Expired,
        (Durability::Durable, OwnerOutcome::Failed) => FeedOutcome::Failed,
    };
    EventBody::Fed {
        feed,
        requested: u8::try_from(result.requested).unwrap_or(u8::MAX),
        completed: u8::try_from(result.completed).unwrap_or(u8::MAX),
        outcome,
        reason: reason(result.reason),
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
        CoreReason::Outlet => FeedReason::Outlet,
        CoreReason::Overcurrent => FeedReason::Overcurrent,
        CoreReason::FeedbackStream => FeedReason::FeedbackStream,
        // No reason is only ever a result still pending, which ends as an
        // output failure if it ends at all.
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
            fed(7, &failed, Durability::Durable, Source::Panel),
            EventBody::Fed {
                feed: 7,
                requested: 2,
                completed: 1,
                outcome: FeedOutcome::Failed,
                reason: FeedReason::NoFeedback,
                by: Source::Panel,
            }
        );

        let done = result(OwnerOutcome::Done, CoreReason::Done);
        let EventBody::Fed { outcome, .. } = fed(7, &done, Durability::Lost, Source::Feeder) else {
            panic!("not a feed");
        };
        assert_eq!(outcome, FeedOutcome::Uncertain);

        let pending = result(OwnerOutcome::Pending, CoreReason::None);
        let EventBody::Fed {
            outcome, reason, ..
        } = fed(7, &pending, Durability::Durable, Source::Feeder)
        else {
            panic!("not a feed");
        };
        assert_eq!(
            (outcome, reason),
            (FeedOutcome::Uncertain, FeedReason::Output)
        );
    }
}

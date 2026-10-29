//! The feeding plans' cache: read on every connect and again after an
//! event that may have changed them, so the next meal the server shows
//! holds while the feeder is offline, and after another client (the app on
//! its own path, the owner's CLI) or the feeder itself changed them. The
//! mirror is the cache's only writer, so a slow read never lands over a
//! newer one.

use tracing::{debug, warn};
use wellfed_connect::Connection;
use wellfed_model::{Answer, EventBody, Op, Plans, Source};

use crate::lanes::Lanes;
use crate::session::End;
use crate::task::Task;

/// Whether `body`, newly stored, may have changed `read`, the plans as
/// last read: a replacement under a newer generation, or a meal of a plan
/// that runs once and is still enabled there, since it disables itself
/// with no `plans` event. History replayed after an outage names older
/// generations and meals the connect's read already reflects, so it asks
/// for no read. With no copy, every such event does.
fn changes_plans(read: Option<&Plans>, body: &EventBody) -> bool {
    let slot = match *body {
        EventBody::Plans { generation, .. } => {
            return read.is_none_or(|plans| plans.generation < generation);
        }
        EventBody::Fed {
            by: Source::Schedule { plan },
            ..
        }
        | EventBody::Missed { plan, .. } => plan,
        _ => return false,
    };
    let Some(plans) = read else {
        return true;
    };
    plans
        .plans
        .get(usize::from(slot))
        .copied()
        .flatten()
        .is_some_and(|plan| plan.enabled && plan.days.once())
}

impl Task {
    /// Whether `body`, newly stored, may have changed the plans this task
    /// last read ([`changes_plans`]).
    pub(crate) fn changes_plans(&self, body: &EventBody) -> bool {
        changes_plans(self.plans.as_ref(), body)
    }

    /// Read the plans into the store's cache. Unlike the connect
    /// sequence's other reads, no answer in time does not end the session:
    /// the cache only goes stale, and the next connect or change reads it
    /// again.
    pub(crate) async fn read_plans(
        &mut self,
        connection: &mut Connection,
        lanes: &mut Lanes,
    ) -> Result<(), End> {
        // Cleared first: a change that arrives during the read marks the
        // copy stale again.
        self.plans_stale = false;

        let answer = self.ask(connection, lanes, &Op::GetPlans).await?;
        if let Some(Answer::Plans(plans)) = answer {
            let now = self.shared.clock.now();
            self.shared.store.record_plans(self.id, &plans, now).await?;
            self.plans = Some(plans);
            return Ok(());
        }

        // The task's copy may be from before an erase, whose generations
        // start again: without it, every `plans` event counts as newer
        // until a read succeeds.
        self.plans = None;
        match answer {
            None => {
                warn!(feeder = %self.id, "no answer to the plans in time; read at the next change")
            }
            Some(Answer::Refused(refusal)) => {
                debug!(feeder = %self.id, "the feeder serves no plans: {refusal:?}");
            }
            Some(other) => warn!(feeder = %self.id, "unexpected answer to the plans: {other:?}"),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;
    use wellfed_model::{
        FeedOutcome, FeedReason, FoodLevel, Generation, MissedReason, PLANS, Plan, Weekdays,
    };

    use super::*;

    fn plans(generation: u32) -> Plans {
        let daily = Plan {
            minute: 7 * 60,
            portions: 1,
            days: Weekdays::DAILY,
            enabled: true,
            call: 0,
        };
        let mut table = [None; PLANS];
        table[0] = Some(daily);
        table[1] = Some(Plan {
            days: Weekdays::ONCE,
            ..daily
        });
        table[2] = Some(Plan {
            days: Weekdays::ONCE,
            enabled: false,
            ..daily
        });
        Plans {
            generation: Generation(generation),
            writable: true,
            plans: table,
        }
    }

    fn replaced(generation: u32) -> EventBody {
        EventBody::Plans {
            generation: Generation(generation),
            by: Source::Feeder,
        }
    }

    fn fed(plan: u8) -> EventBody {
        EventBody::Fed {
            feed: 1,
            requested: 1,
            completed: 1,
            outcome: FeedOutcome::Done,
            reason: FeedReason::Done,
            hopper: FoodLevel::Present,
            by: Source::Schedule { plan },
        }
    }

    #[test]
    fn only_a_change_the_copy_does_not_hold_asks_for_a_read() {
        let read = plans(5);
        let missed = EventBody::Missed {
            plan: 1,
            due: Timestamp::UNIX_EPOCH,
            reason: MissedReason::Late,
        };
        let cases = [
            (
                "a replayed older generation",
                Some(&read),
                replaced(4),
                false,
            ),
            ("the generation it holds", Some(&read), replaced(5), false),
            ("a newer generation", Some(&read), replaced(6), true),
            ("no copy", None, replaced(1), true),
            ("no copy, a scheduled meal", None, fed(0), true),
            ("a once plan's meal", Some(&read), fed(1), true),
            ("a once plan's missed meal", Some(&read), missed, true),
            ("a once plan already disabled", Some(&read), fed(2), false),
            ("a daily plan's meal", Some(&read), fed(0), false),
            ("an empty slot", Some(&read), fed(7), false),
            ("anything else", None, EventBody::Lost { count: 1 }, false),
        ];
        for (case, copy, body, read_again) in cases {
            assert_eq!(changes_plans(copy, &body), read_again, "{case}");
        }
    }
}

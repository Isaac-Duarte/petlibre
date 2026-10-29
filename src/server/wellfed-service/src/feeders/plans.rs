//! The feeding plans: read from the feeder and replaced as one table
//! against the generation read, as the settings are, and the next meal the
//! server reckons from the copy it holds.
//!
//! The copy is the mirror's alone (`wellfed-mirror`): it reads the plans on
//! every connect and after each event that may have changed them, a change
//! made here included. The reads here answer the caller and record nothing,
//! so an older answer never lands over a newer one.

use serde::Serialize;
use wellfed_api::{
    CommandKind, CommandOutcome, FeederId, FeederRefusal, Generation, NextMeal, Plans, PlansChange,
};
use wellfed_model::{Answer, Op, PLANS, Plan};

use crate::feeders::commands::{CommandError, refused};
use crate::feeders::journal::Write;
use crate::{Caller, Error, IdempotencyKey, InternalError, Invalid, Result, Service};

/// A plans change as it is hashed into the journal: the base it names is
/// part of the request.
#[derive(Serialize)]
struct PlansWrite {
    base: Generation,
    plans: [Option<Plan>; PLANS],
}

impl Service {
    /// The plans, read from the feeder: a change must name the current
    /// generation, so a copy would only invite a stale write.
    pub async fn plans(&self, id: FeederId) -> Result<Plans> {
        self.row(id).await?;
        self.read_plans(id).await
    }

    /// Replace the plans if they are still at `base`. A stale base gets
    /// [`Error::StalePlans`] with the current plans. After the change the
    /// plans are read again and kept as the write's outcome, so a retry
    /// under the same key returns them; if that read fails the change still
    /// happened, and a retry reads them again.
    pub async fn put_plans(
        &self,
        caller: &Caller,
        id: FeederId,
        base: Generation,
        change: PlansChange,
        key: Option<IdempotencyKey>,
    ) -> Result<Plans> {
        // `PLANS` slots, numbered from 0: every slot gets its number.
        let invalid = change
            .plans
            .iter()
            .zip(0u8..)
            .find_map(|(plan, slot)| plan.filter(|plan| !plan.valid()).map(|_| slot));
        if let Some(slot) = invalid {
            return Err(Invalid::Plan { slot }.into());
        }
        self.live_row(id).await?;

        let body = PlansWrite {
            base,
            plans: change.plans,
        };
        let recorded = self
            .journaled(Write {
                caller,
                feeder: id,
                kind: CommandKind::Plans,
                body: &body,
                key,
                op: Op::PutPlans {
                    base,
                    plans: change.plans,
                },
            })
            .await?;

        match recorded.outcome {
            CommandOutcome::Plans { plans } => Ok(plans),
            // As for settings: the change is made, so a failed read-back is
            // unknown rather than offline, and a retry reads them again.
            CommandOutcome::Done => {
                let plans = self.read_plans(id).await.map_err(|error| match error {
                    Error::Command(CommandError::Offline) => CommandError::Unknown.into(),
                    other => other,
                })?;
                self.amend(&recorded.key, &CommandOutcome::Plans { plans })
                    .await?;
                Ok(plans)
            }
            // The current plans, read now, as a stale settings change gets
            // the current settings.
            CommandOutcome::Refused {
                refusal: FeederRefusal::Stale,
            } => Err(Error::StalePlans(self.read_plans(id).await?)),
            CommandOutcome::Refused { refusal } => Err(Error::Refused(refusal)),
            CommandOutcome::Settings { .. }
            | CommandOutcome::Feeding { .. }
            | CommandOutcome::Fed { .. }
            | CommandOutcome::Invited { .. }
            | CommandOutcome::Unrecorded => Err(InternalError::Unexpected.into()),
        }
    }

    /// The next meal of `plans`, the copy the server holds, in the
    /// household's zone; `None` with no household, no plans read or none
    /// enabled.
    pub(crate) fn next_meal(&self, plans: Option<&Plans>) -> Option<NextMeal> {
        let zone = self.mirror.zone()?;
        zone.next_meal(plans?, self.shared.clock.now())
    }

    async fn read_plans(&self, id: FeederId) -> Result<Plans> {
        match self.ask(id, self.request_id()?, Op::GetPlans).await? {
            Answer::Plans(plans) => Ok(plans),
            other => Err(refused(&other)),
        }
    }
}

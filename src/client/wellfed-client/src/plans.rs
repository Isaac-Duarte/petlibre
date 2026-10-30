//! Changing the feeding plans, and what they come to today. The feeder
//! replaces the whole table against a generation, so a change is a read, an
//! edit of that table ([`edited`]) and a write; every client makes the same
//! edit here, and a stale write is refused rather than merged.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use wellfed_model::{CALL_PLAYS_MAX, PLANS, PORTIONS_MAX, Plan, Plans, TimeOfDay, Weekdays};

use crate::history::Feed;
use crate::time::next_meal;

/// What a change does to the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    /// Into the first free slot.
    Add(Plan),
    /// What it does not name stays.
    Change {
        slot: u8,
        at: Option<TimeOfDay>,
        portions: Option<u8>,
        days: Option<Weekdays>,
        call: Option<u8>,
    },
    Enabled {
        slot: u8,
        enabled: bool,
    },
    Delete {
        slot: u8,
    },
}

/// Why an edit cannot be made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    #[error("slot {slot} holds no plan")]
    Empty { slot: u8 },
    #[error("every one of the {PLANS} slots holds a plan")]
    Full,
    #[error("the change names nothing to change")]
    Nothing,
    #[error(
        "a plan feeds at a minute of the day, 1 to {PORTIONS_MAX} portions, with up to {CALL_PLAYS_MAX} calls"
    )]
    Invalid,
}

/// The table `plans` holds with `edit` applied.
pub fn edited(plans: &Plans, edit: Edit) -> Result<[Option<Plan>; PLANS], EditError> {
    let mut table = plans.plans;
    let filled = |table: &[Option<Plan>; PLANS], slot: u8| {
        table
            .get(usize::from(slot))
            .copied()
            .flatten()
            .ok_or(EditError::Empty { slot })
    };
    let checked = |plan: Plan| {
        if plan.valid() {
            Ok(plan)
        } else {
            Err(EditError::Invalid)
        }
    };

    match edit {
        Edit::Add(plan) => {
            let free = table
                .iter_mut()
                .find(|slot| slot.is_none())
                .ok_or(EditError::Full)?;
            *free = Some(checked(plan)?);
        }
        Edit::Change {
            slot,
            at,
            portions,
            days,
            call,
        } => {
            let mut plan = filled(&table, slot)?;
            if at.is_none() && portions.is_none() && days.is_none() && call.is_none() {
                return Err(EditError::Nothing);
            }
            plan.minute = at.map_or(plan.minute, |at| at.0);
            plan.portions = portions.unwrap_or(plan.portions);
            plan.days = days.unwrap_or(plan.days);
            plan.call = call.unwrap_or(plan.call);
            table[usize::from(slot)] = Some(checked(plan)?);
        }
        Edit::Enabled { slot, enabled } => {
            let plan = filled(&table, slot)?;
            table[usize::from(slot)] = Some(Plan { enabled, ..plan });
        }
        Edit::Delete { slot } => {
            filled(&table, slot)?;
            table[usize::from(slot)] = None;
        }
    }
    Ok(table)
}

/// How one of today's meals stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MealState {
    /// Its plan fed today: the feed's result.
    Fed(Feed),
    /// Its time has passed with no feed recorded for it: late, missed, or
    /// the plan changed after the feed that ran.
    Passed,
    /// The next meal of all the plans.
    Next {
        at: Timestamp,
    },
    Later,
}

/// One enabled plan that runs today, by the feeder's local calendar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Meal {
    pub slot: u8,
    pub minute: u16,
    pub portions: u8,
    pub call: u8,
    pub state: MealState,
}

/// Today's meals in `zone`, earliest first. `feeds` is the history as far as
/// it was read: each meal takes the last feed its plan's slot fed today, at
/// or after its minute, so a plan moved later today shows nothing as fed.
pub fn today(plans: &Plans, feeds: &[Feed], now: Timestamp, zone: &TimeZone) -> Vec<Meal> {
    let local = now.to_zoned(zone.clone());
    let day = local.date();
    let minute_now =
        u16::try_from(i32::from(local.hour()) * 60 + i32::from(local.minute())).unwrap_or(u16::MAX);
    let next = next_meal(plans, now, zone);

    let mut meals: Vec<Meal> = plans
        .plans
        .iter()
        .enumerate()
        .filter_map(|(slot, plan)| {
            let plan = (*plan)?;
            let slot = u8::try_from(slot).ok()?;
            let runs = plan.days.bits() == 0 || runs_on(plan.days, day);
            // A once plan that ran stays in its slot disabled: it still
            // shows as today's meal when its feed is today's.
            let fed = last_feed(feeds, slot, plan.minute, zone, day);
            if !(plan.enabled && runs) && fed.is_none() {
                return None;
            }

            let state = match (fed, next) {
                (Some(feed), _) => MealState::Fed(feed),
                (None, Some(next)) if next.slot == slot && same_day(next.at, zone, day) => {
                    MealState::Next { at: next.at }
                }
                (None, _) if plan.minute <= minute_now => MealState::Passed,
                (None, _) => MealState::Later,
            };
            Some(Meal {
                slot,
                minute: plan.minute,
                portions: plan.portions,
                call: plan.call,
                state,
            })
        })
        .collect();
    meals.sort_by_key(|meal| (meal.minute, meal.slot));
    meals
}

fn runs_on(days: Weekdays, day: jiff::civil::Date) -> bool {
    let bit = u32::try_from(day.weekday().to_monday_zero_offset()).unwrap_or(0);
    u32::from(days.bits()) & (1 << bit) != 0
}

fn same_day(at: Timestamp, zone: &TimeZone, day: jiff::civil::Date) -> bool {
    at.to_zoned(zone.clone()).date() == day
}

fn last_feed(
    feeds: &[Feed],
    slot: u8,
    minute: u16,
    zone: &TimeZone,
    day: jiff::civil::Date,
) -> Option<Feed> {
    feeds.iter().rev().copied().find(|feed| {
        let ours =
            matches!(feed.by, Some(wellfed_model::Source::Schedule { plan }) if plan == slot);
        ours && feed.at.is_some_and(|at| {
            let local = at.to_zoned(zone.clone());
            let at_minute = i32::from(local.hour()) * 60 + i32::from(local.minute());
            local.date() == day && at_minute >= i32::from(minute)
        })
    })
}

#[cfg(test)]
mod tests {
    use wellfed_model::{FeedOutcome, FeedReason, FoodLevel, Generation, Source};

    use super::*;
    use crate::history::Delivered;

    fn plans(table: [Option<Plan>; PLANS]) -> Plans {
        Plans {
            generation: Generation(3),
            writable: true,
            plans: table,
        }
    }

    const BREAKFAST: Plan = Plan {
        minute: 7 * 60 + 30,
        portions: 2,
        days: Weekdays::DAILY,
        enabled: true,
        call: 0,
    };

    #[test]
    fn a_new_plan_takes_the_first_free_slot() {
        let mut table = [None; PLANS];
        table[0] = Some(BREAKFAST);
        let table = edited(&plans(table), Edit::Add(BREAKFAST)).unwrap();
        assert_eq!(table[1], Some(BREAKFAST));

        let full = plans([Some(BREAKFAST); PLANS]);
        assert_eq!(edited(&full, Edit::Add(BREAKFAST)), Err(EditError::Full));
    }

    #[test]
    fn an_edit_keeps_what_it_does_not_name() {
        let mut table = [None; PLANS];
        table[4] = Some(BREAKFAST);
        let change = Edit::Change {
            slot: 4,
            at: None,
            portions: Some(3),
            days: None,
            call: Some(2),
        };
        let table = edited(&plans(table), change).unwrap();
        assert_eq!(
            table[4],
            Some(Plan {
                portions: 3,
                call: 2,
                ..BREAKFAST
            })
        );
    }

    #[test]
    fn an_empty_slot_cannot_be_changed() {
        let table = plans([None; PLANS]);
        for edit in [
            Edit::Delete { slot: 2 },
            Edit::Enabled {
                slot: 2,
                enabled: true,
            },
        ] {
            assert_eq!(edited(&table, edit), Err(EditError::Empty { slot: 2 }));
        }
    }

    #[test]
    fn a_change_that_names_nothing_or_breaks_a_limit_is_refused() {
        let mut table = [None; PLANS];
        table[1] = Some(BREAKFAST);
        let change = |portions, call| Edit::Change {
            slot: 1,
            at: None,
            portions,
            days: None,
            call,
        };
        assert_eq!(
            edited(&plans(table), change(None, None)),
            Err(EditError::Nothing)
        );
        assert_eq!(
            edited(&plans(table), change(Some(49), None)),
            Err(EditError::Invalid)
        );
        assert_eq!(
            edited(&plans(table), change(None, Some(6))),
            Err(EditError::Invalid)
        );
    }

    fn fed(slot: u8, at: Timestamp) -> Feed {
        Feed {
            epoch: 1,
            seq: 9,
            feed: 4,
            at: Some(at),
            by: Some(Source::Schedule { plan: slot }),
            requested: 2,
            delivered: Delivered::Counted(2),
            outcome: FeedOutcome::Done,
            reason: FeedReason::Done,
            hopper: FoodLevel::Present,
        }
    }

    #[test]
    fn today_says_how_each_meal_stands() {
        let zone = TimeZone::UTC;
        // A Friday, 12:04.
        let now: Timestamp = "2026-10-30T12:04:00Z".parse().unwrap();
        let mut table = [None; PLANS];
        table[0] = Some(BREAKFAST);
        table[1] = Some(Plan {
            minute: 12 * 60 + 30,
            ..BREAKFAST
        });
        table[2] = Some(Plan {
            minute: 18 * 60 + 30,
            portions: 3,
            call: 1,
            ..BREAKFAST
        });
        // Mondays only, so not today's.
        table[3] = Some(Plan {
            minute: 9 * 60,
            days: Weekdays::from_bits(1),
            ..BREAKFAST
        });
        table[4] = Some(Plan {
            minute: 8 * 60,
            ..BREAKFAST
        });
        table[5] = Some(Plan {
            minute: 6 * 60,
            enabled: false,
            ..BREAKFAST
        });

        let breakfast = fed(0, "2026-10-30T07:30:40Z".parse().unwrap());
        // Yesterday's feed of slot 4 is not today's.
        let yesterday = fed(4, "2026-10-29T08:00:40Z".parse().unwrap());
        let meals = today(&plans(table), &[yesterday, breakfast], now, &zone);

        let states: Vec<_> = meals.iter().map(|meal| (meal.slot, meal.state)).collect();
        assert_eq!(
            states,
            [
                (0, MealState::Fed(breakfast)),
                (4, MealState::Passed),
                (
                    1,
                    MealState::Next {
                        at: "2026-10-30T12:30:00Z".parse().unwrap()
                    }
                ),
                (2, MealState::Later),
            ]
        );
    }

    #[test]
    fn a_plan_that_ran_once_still_shows_todays_feed() {
        let zone = TimeZone::UTC;
        let now: Timestamp = "2026-10-30T12:04:00Z".parse().unwrap();
        let mut table = [None; PLANS];
        table[0] = Some(Plan {
            days: Weekdays::ONCE,
            enabled: false,
            ..BREAKFAST
        });
        let once = fed(0, "2026-10-30T07:30:40Z".parse().unwrap());

        let meals = today(&plans(table), &[once], now, &zone);
        assert_eq!(meals.len(), 1);
        assert_eq!(meals[0].state, MealState::Fed(once));
    }
}

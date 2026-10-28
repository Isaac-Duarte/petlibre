//! The feeding plans over the device API: list them, or add, edit, enable,
//! disable or delete one. A change reads the plans, edits that table and
//! writes it back against the generation it read, so a change made
//! meanwhile (another client's, or a plan that ran once disabling itself)
//! is refused rather than overwritten; it is never retried.

use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use wellfed_model::{
    Answer, Op, PLAN_PORTIONS_MAX, PLANS, Plan, Plans, Refusal, TimeOfDay, Weekdays,
};

use crate::{Target, ask_and_print, connect, print_answer};

#[derive(Args)]
pub(crate) struct PlansArgs {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// The plan in each slot.
    List(Target),
    /// Add a plan in the first free slot.
    Add {
        #[command(flatten)]
        target: Target,
        /// The feeder's local time to feed at, HH:MM.
        #[arg(long, value_name = "HH:MM")]
        at: TimeOfDay,
        #[arg(long, value_parser = portions())]
        portions: u8,
        /// `daily`, `weekdays`, `weekends`, `once` (runs at its next
        /// occurrence, then disables itself) or a list like `mon,thu`.
        #[arg(long, default_value = "daily")]
        days: Weekdays,
        /// Store it disabled.
        #[arg(long)]
        disabled: bool,
    },
    /// Change a plan's time, portions or days. A new time or days runs
    /// only from the next occurrence after the change.
    Edit {
        #[command(flatten)]
        target: Target,
        #[arg(value_parser = slot())]
        slot: u8,
        #[arg(long, value_name = "HH:MM")]
        at: Option<TimeOfDay>,
        #[arg(long, value_parser = portions())]
        portions: Option<u8>,
        #[arg(long)]
        days: Option<Weekdays>,
    },
    Enable {
        #[command(flatten)]
        target: Target,
        #[arg(value_parser = slot())]
        slot: u8,
    },
    Disable {
        #[command(flatten)]
        target: Target,
        #[arg(value_parser = slot())]
        slot: u8,
    },
    /// Empty a plan's slot.
    Delete {
        #[command(flatten)]
        target: Target,
        #[arg(value_parser = slot())]
        slot: u8,
    },
}

fn slot() -> clap::builder::RangedI64ValueParser<u8> {
    clap::value_parser!(u8).range(0..i64::try_from(PLANS).unwrap_or(i64::MAX))
}

fn portions() -> clap::builder::RangedI64ValueParser<u8> {
    clap::value_parser!(u8).range(1..=i64::from(PLAN_PORTIONS_MAX))
}

/// What a change does to the table.
#[derive(Clone, Copy)]
enum Edit {
    Add(Plan),
    Change {
        slot: u8,
        at: Option<TimeOfDay>,
        portions: Option<u8>,
        days: Option<Weekdays>,
    },
    Enabled {
        slot: u8,
        enabled: bool,
    },
    Delete {
        slot: u8,
    },
}

pub(crate) async fn command(args: PlansArgs) -> Result<()> {
    let (target, edit) = match args.action {
        Action::List(target) => return ask_and_print(&target, &Op::GetPlans).await,
        Action::Add {
            target,
            at,
            portions,
            days,
            disabled,
        } => {
            let plan = Plan {
                minute: at.0,
                portions,
                days,
                enabled: !disabled,
            };
            (target, Edit::Add(plan))
        }
        Action::Edit {
            target,
            slot,
            at,
            portions,
            days,
        } => {
            let change = Edit::Change {
                slot,
                at,
                portions,
                days,
            };
            (target, change)
        }
        Action::Enable { target, slot } => (
            target,
            Edit::Enabled {
                slot,
                enabled: true,
            },
        ),
        Action::Disable { target, slot } => (
            target,
            Edit::Enabled {
                slot,
                enabled: false,
            },
        ),
        Action::Delete { target, slot } => (target, Edit::Delete { slot }),
    };

    let (_, _, mut connection) = connect(&target).await?;
    let current = match connection.ask(&Op::GetPlans).await? {
        Answer::Plans(plans) => plans,
        Answer::Refused(Refusal::Unavailable) => bail!("this feeder keeps no plans"),
        answer => {
            print_answer(&answer)?;
            bail!("the feeder did not return its plans");
        }
    };
    let plans = edited(&current, edit)?;
    let answer = connection
        .ask(&Op::PutPlans {
            base: current.generation,
            plans,
        })
        .await?;
    if let Answer::Refused(Refusal::Stale { .. }) = answer {
        eprintln!("The plans changed since they were read; read them again and retry.");
    }
    print_answer(&answer)
}

/// The table `plans` holds with `edit` applied.
fn edited(plans: &Plans, edit: Edit) -> Result<[Option<Plan>; PLANS]> {
    let mut table = plans.plans;
    let filled = |table: &[Option<Plan>; PLANS], slot: u8| -> Result<Plan> {
        match table.get(usize::from(slot)).copied().flatten() {
            Some(plan) => Ok(plan),
            None => bail!("slot {slot} holds no plan"),
        }
    };

    match edit {
        Edit::Add(plan) => {
            let Some(free) = table.iter_mut().find(|slot| slot.is_none()) else {
                bail!("every one of the {PLANS} slots holds a plan");
            };
            *free = Some(plan);
        }
        Edit::Change {
            slot,
            at,
            portions,
            days,
        } => {
            let mut plan = filled(&table, slot)?;
            if at.is_none() && portions.is_none() && days.is_none() {
                bail!("pass --at, --portions or --days");
            }
            plan.minute = at.map_or(plan.minute, |at| at.0);
            plan.portions = portions.unwrap_or(plan.portions);
            plan.days = days.unwrap_or(plan.days);
            table[usize::from(slot)] = Some(plan);
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

#[cfg(test)]
mod tests {
    use super::*;
    use wellfed_model::Generation;

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
    };

    #[test]
    fn a_new_plan_takes_the_first_free_slot() {
        let mut table = [None; PLANS];
        table[0] = Some(BREAKFAST);
        let table = edited(&plans(table), Edit::Add(BREAKFAST)).unwrap();
        assert_eq!(table[1], Some(BREAKFAST));

        let full = plans([Some(BREAKFAST); PLANS]);
        assert!(edited(&full, Edit::Add(BREAKFAST)).is_err());
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
        };
        let table = edited(&plans(table), change).unwrap();
        assert_eq!(
            table[4],
            Some(Plan {
                portions: 3,
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
            assert!(edited(&table, edit).is_err());
        }
    }
}

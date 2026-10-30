//! The feeding plans over the device API: list them, or add, edit, enable,
//! disable or delete one. A change reads the plans, edits that table and
//! writes it back against the generation it read, so a change made
//! meanwhile (another client's, or a plan that ran once disabling itself)
//! is refused rather than overwritten; it is never retried.
//!
//! `wellfed server plans` makes the same changes through the server, from
//! the same arguments ([`NewPlan`], [`PlanChange`], [`Slot`]) and with the
//! same edit (`wellfed_client::plans::edited`).

use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use wellfed_client::plans::{Edit, edited};
use wellfed_model::{
    Answer, CALL_PLAYS_MAX, Op, PLANS, PORTIONS_MAX, Plan, Refusal, TimeOfDay, Weekdays,
};

use crate::{Target, connect, print_answer, text};

#[derive(Args)]
pub(crate) struct PlansArgs {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// The plan in each slot.
    List {
        #[command(flatten)]
        target: Target,
        /// The plans as JSON, as the wire spells them, instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Add a plan in the first free slot.
    Add {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        plan: NewPlan,
    },
    /// Change a plan's time, portions, days or meal call. A new time or
    /// days runs only from the next occurrence after the change.
    Edit {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        change: PlanChange,
    },
    /// Enable a plan: it runs from its next occurrence.
    Enable {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        slot: Slot,
    },
    /// Disable a plan; it keeps its slot.
    Disable {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        slot: Slot,
    },
    /// Empty a plan's slot.
    Delete {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        slot: Slot,
    },
}

/// A plan to add.
#[derive(Args)]
pub(crate) struct NewPlan {
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
    /// Play the meal call this many times before the meal (0 to 5;
    /// `wellfed call upload` stores the clip).
    #[arg(long, default_value_t = 0, value_parser = plays())]
    call: u8,
}

impl NewPlan {
    pub(crate) const fn edit(&self) -> Edit {
        Edit::Add(Plan {
            minute: self.at.0,
            portions: self.portions,
            days: self.days,
            enabled: !self.disabled,
            call: self.call,
        })
    }
}

/// What to change in one plan; what it does not name stays.
#[derive(Args)]
pub(crate) struct PlanChange {
    #[arg(value_parser = slot())]
    slot: u8,
    #[arg(long, value_name = "HH:MM")]
    at: Option<TimeOfDay>,
    #[arg(long, value_parser = portions())]
    portions: Option<u8>,
    #[arg(long)]
    days: Option<Weekdays>,
    /// The meal call's plays before the meal, 0 for none.
    #[arg(long, value_parser = plays())]
    call: Option<u8>,
}

impl PlanChange {
    pub(crate) const fn edit(&self) -> Edit {
        Edit::Change {
            slot: self.slot,
            at: self.at,
            portions: self.portions,
            days: self.days,
            call: self.call,
            enabled: None,
        }
    }
}

/// A plan's slot, 0-9.
#[derive(Args)]
pub(crate) struct Slot {
    #[arg(value_parser = slot())]
    slot: u8,
}

impl Slot {
    pub(crate) const fn enabled(&self, enabled: bool) -> Edit {
        Edit::Enabled {
            slot: self.slot,
            enabled,
        }
    }

    pub(crate) const fn delete(&self) -> Edit {
        Edit::Delete { slot: self.slot }
    }
}

fn slot() -> clap::builder::RangedI64ValueParser<u8> {
    clap::value_parser!(u8).range(0..i64::try_from(PLANS).unwrap_or(i64::MAX))
}

/// Portions for a feed or a plan: 1 to the feeder's cap, which it checks
/// again and refuses by name.
pub(crate) fn portions() -> clap::builder::RangedI64ValueParser<u8> {
    clap::value_parser!(u8).range(1..=i64::from(PORTIONS_MAX))
}

/// The meal call's plays before a meal.
fn plays() -> clap::builder::RangedI64ValueParser<u8> {
    clap::value_parser!(u8).range(0..=i64::from(CALL_PLAYS_MAX))
}

pub(crate) async fn command(args: PlansArgs) -> Result<()> {
    let (target, edit) = match args.action {
        Action::List { target, json } => return list(&target, json).await,
        Action::Add { target, plan } => (target, plan.edit()),
        Action::Edit { target, change } => (target, change.edit()),
        Action::Enable { target, slot } => (target, slot.enabled(true)),
        Action::Disable { target, slot } => (target, slot.enabled(false)),
        Action::Delete { target, slot } => (target, slot.delete()),
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

/// The plans, a line a filled slot, or as JSON.
async fn list(target: &Target, json: bool) -> Result<()> {
    let (_, _, mut connection) = connect(target).await?;
    match connection.ask(&Op::GetPlans).await? {
        Answer::Plans(plans) if !json => {
            println!("{}", text::plans(&plans));
            Ok(())
        }
        answer => print_answer(&answer),
    }
}

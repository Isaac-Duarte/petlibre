//! The feeding plans through the server: the same verbs and edit as
//! `wellfed plans` ([`crate::plans`]), so both paths change a table alike.
//! A change reads the plans, edits that table and writes it back once,
//! under a fresh key, against the generation it read: a change made
//! meanwhile is refused with the current plans, never overwritten.

use anyhow::Result;
use clap::{Args, Subcommand};
use uuid::Uuid;
use wellfed_server_client::Client;
use wellfed_server_client::api::PlansChange;

use super::feeders::Which;
use super::{Output, Settled, Written, write};
use wellfed_client::plans::edited;

use crate::plans::{NewPlan, PlanChange, Slot};

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
        which: Which,
    },
    /// Add a plan in the first free slot.
    Add {
        #[command(flatten)]
        which: Which,
        #[command(flatten)]
        plan: NewPlan,
    },
    /// Change a plan's time, portions, days or meal call. A new time or
    /// days runs only from the next occurrence after the change.
    Edit {
        #[command(flatten)]
        which: Which,
        #[command(flatten)]
        change: PlanChange,
    },
    /// Enable a plan: it runs from its next occurrence.
    Enable {
        #[command(flatten)]
        which: Which,
        #[command(flatten)]
        slot: Slot,
    },
    /// Disable a plan; it keeps its slot.
    Disable {
        #[command(flatten)]
        which: Which,
        #[command(flatten)]
        slot: Slot,
    },
    /// Empty a plan's slot.
    Delete {
        #[command(flatten)]
        which: Which,
        #[command(flatten)]
        slot: Slot,
    },
}

pub(super) async fn plans(client: &Client, args: PlansArgs, out: Output) -> Result<Settled> {
    let (which, edit) = match args.action {
        Action::List { which } => {
            let id = which.id(client).await?;
            let plans = client.plans(id).await?;
            out.show(&plans, crate::text::plans)?;
            return Ok(Settled::Known);
        }
        Action::Add { which, plan } => (which, plan.edit()),
        Action::Edit { which, change } => (which, change.edit()),
        Action::Enable { which, slot } => (which, slot.enabled(true)),
        Action::Disable { which, slot } => (which, slot.enabled(false)),
        Action::Delete { which, slot } => (which, slot.delete()),
    };

    let id = which.id(client).await?;
    let current = client.plans(id).await?;
    let change = PlansChange {
        plans: edited(&current, edit)?,
    };

    let key = Uuid::new_v4();
    let sent = client.put_plans(id, current.generation, &change, key);
    let Written::Answered(plans) = write(out, key, sent).await? else {
        return Ok(Settled::Unknown);
    };
    out.show(&plans, crate::text::plans)?;
    Ok(Settled::Known)
}

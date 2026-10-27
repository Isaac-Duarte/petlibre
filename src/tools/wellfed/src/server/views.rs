//! What the server works out from its data: the alerts it raised, a
//! feeder's food left and a pet's intake. Everyone reads them; only an
//! owner acknowledges an alert.

use anyhow::{Result, bail};
use clap::{Subcommand, ValueEnum};
use jiff::civil::Date;
use wellfed_server_client::Client;
use wellfed_server_client::api::{
    AlertId, AlertQuery, AlertState, FeederFood, IntakeQuery, Pet, PetId, PetIntake,
};
use uuid::Uuid;

use super::feeders::Which;
use super::{Output, Settled, text};

#[derive(Subcommand)]
pub(crate) enum Alerts {
    /// The alerts, oldest first.
    List {
        /// Only the alerts in this state.
        #[arg(long, value_enum)]
        state: Option<StateArg>,
        /// Only this feeder's alerts (its id or name).
        #[arg(long)]
        feeder: Option<String>,
        /// The previous page's `next`, as it came.
        #[arg(long)]
        after: Option<String>,
        /// At most this many (1-1000, default 100).
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Acknowledge an alert. Owners only.
    Ack { id: Uuid },
}

/// An alert's state, as `--state` takes it.
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum StateArg {
    Open,
    Acknowledged,
    Cleared,
}

impl From<StateArg> for AlertState {
    fn from(state: StateArg) -> Self {
        match state {
            StateArg::Open => Self::Open,
            StateArg::Acknowledged => Self::Acknowledged,
            StateArg::Cleared => Self::Cleared,
        }
    }
}

pub(super) async fn alerts(client: &Client, alerts: Alerts, out: Output) -> Result<Settled> {
    match alerts {
        Alerts::List {
            state,
            feeder,
            after,
            limit,
        } => {
            let feeder = match feeder {
                Some(feeder) => Some(Which::named(feeder).id(client).await?),
                None => None,
            };
            let query = AlertQuery {
                state: state.map(AlertState::from),
                feeder,
                after,
                limit,
            };
            let page = client.alerts(&query).await?;
            out.show(&page, |page| text::lines(&page.items, text::alert))?;
            if let Some(next) = &page.next {
                eprintln!("More after {next}: --after {next}");
            }
        }
        Alerts::Ack { id } => {
            // Acknowledging twice is harmless, so it needs no key.
            let alert = client.acknowledge_alert(AlertId(id)).await?;
            out.show(&alert, text::alert)?;
        }
    }
    Ok(Settled::Known)
}

pub(super) async fn food(client: &Client, which: &Which, out: Output) -> Result<Settled> {
    let id = which.id(client).await?;

    let food = client.feeder_food(id).await?;
    out.show(&food, food_text)?;
    Ok(Settled::Known)
}

fn food_text(food: &FeederFood) -> String {
    let mut lines = vec![match &food.current {
        Some(period) => format!(
            "Food: {} {} ({} kcal/kg, {} g a cup) since {}",
            period.food.brand,
            period.food.name,
            period.food.kcal_per_kg,
            period.food.grams_per_cup,
            period.from
        ),
        None => "Food: none set".to_owned(),
    }];
    lines.push(match &food.hopper {
        Some(hopper) if hopper.present => format!("Hopper: food read at {}", hopper.received),
        Some(hopper) => format!("Hopper: EMPTY, read at {}", hopper.received),
        None => "Hopper: not read yet".to_owned(),
    });
    match (&food.last_refill, &food.estimate) {
        (Some(refill), Some(estimate)) => {
            lines.push(format!("Refilled {} g at {}", refill.grams, refill.at));
            lines.push(format!(
                "Since: {} portions, {} g; about {} g left (an estimate){}",
                estimate.portions,
                estimate.dispensed_grams,
                estimate.remaining_grams,
                estimate
                    .empty_by
                    .map(|at| format!(", empty by {at}"))
                    .unwrap_or_default()
            ));
        }
        _ => lines.push("No refill recorded".to_owned()),
    }
    if let Some(at) = food.unrecorded_refill {
        lines.push(format!("Refilled? The beam saw food return at {at}"));
    }
    lines.join("\n")
}

pub(super) async fn intake(
    client: &Client,
    pet: &str,
    from: Option<Date>,
    to: Option<Date>,
    out: Output,
) -> Result<Settled> {
    let pet = pet_id(client, pet).await?;

    let intake = client.pet_intake(pet, &IntakeQuery { from, to }).await?;
    out.show(&intake, intake_text)?;
    Ok(Settled::Known)
}

/// The pet a name or an id names.
async fn pet_id(client: &Client, which: &str) -> Result<PetId> {
    let pets = client.pets().await?;
    let matching: Vec<&Pet> = pets
        .iter()
        .filter(|pet| pet.name == which || pet.id.to_string() == which.to_lowercase())
        .collect();
    let [pet] = matching.as_slice() else {
        bail!("{} pets are named or numbered {which:?}", matching.len());
    };
    Ok(pet.id)
}

fn intake_text(intake: &PetIntake) -> String {
    let mut lines: Vec<String> = intake
        .days
        .iter()
        .map(|day| {
            format!(
                "{}  {} g  {} kcal{}{}",
                day.date,
                day.grams,
                day.kcal,
                day.target_percent
                    .map(|percent| format!("  {percent}% of target"))
                    .unwrap_or_default(),
                if day.estimate {
                    "  (shared: estimate)"
                } else {
                    ""
                },
            )
        })
        .collect();
    lines.extend(intake.anomalies.iter().map(|anomaly| {
        format!(
            "{}  feed {} on {}: {} ({} of {} portions, {})",
            anomaly.at,
            anomaly.feed,
            anomaly.feeder,
            text::word(&anomaly.kind),
            anomaly.completed,
            anomaly.requested,
            text::word(&anomaly.reason),
        )
    }));
    lines.join("\n")
}

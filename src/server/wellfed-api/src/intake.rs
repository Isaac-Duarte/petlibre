//! `GET /pets/{id}/intake`: what a pet's feeders dispensed for it, day by
//! day in the household's zone, against its daily target, and the feeds
//! that went wrong.
//!
//! The feeders cannot tell who ate, so a feed from a feeder that served
//! several pets at the time is split evenly between them, and the day is
//! marked an estimate. A feed counts the portions the feeder confirmed,
//! weighed as 1/12 cup of the food the feeder served then (provisional).

use jiff::Timestamp;
use jiff::civil::Date;
use wellfed_model::FeedReason;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::{FeederId, PetId};

/// The most days one request covers.
pub const INTAKE_DAYS_MAX: u32 = 366;

/// Which days to read, in the household's zone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct IntakeQuery {
    /// The first day; six days before `to` when absent.
    #[param(value_type = Option<String>, format = Date)]
    pub from: Option<Date>,
    /// The last day, included; today when absent. At most 366 days from
    /// `from`.
    #[param(value_type = Option<String>, format = Date)]
    pub to: Option<Date>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PetIntake {
    pub pet: PetId,
    /// The pet's daily target now.
    pub daily_kcal: Option<u32>,
    /// Every day asked for, oldest first, empty days included.
    pub days: Vec<IntakeDay>,
    /// The feeds of the pet's feeders in those days that went wrong,
    /// oldest first.
    pub anomalies: Vec<FeedAnomaly>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct IntakeDay {
    #[schema(value_type = String, format = Date)]
    pub date: Date,
    pub grams: u32,
    pub kcal: u32,
    /// `kcal` as a percentage of the daily target; `None` without one.
    pub target_percent: Option<u32>,
    /// Part of the day came from a feeder that served other pets too, and
    /// was split evenly between them.
    pub estimate: bool,
    /// This pet's share of portions from feeds with no food set, which
    /// weigh nothing known, in thousandths of a portion.
    pub unweighed_milliportions: u32,
}

/// A feed that went wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FeedAnomaly {
    pub feeder: FeederId,
    pub epoch: u32,
    pub feed: u32,
    /// When it ended: the feeder's time when its clock was valid, else
    /// when the server received the result.
    pub at: Timestamp,
    pub kind: AnomalyKind,
    pub requested: u8,
    pub completed: u8,
    pub reason: FeedReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnomalyKind {
    /// It never started (`expired`): the feeder could not get ready.
    Missed,
    /// The feeder stopped without recording a result: anything up to the
    /// portions asked for may have dropped.
    Uncertain,
    /// It gave fewer portions than asked for (`failed`, or done short).
    Short,
}

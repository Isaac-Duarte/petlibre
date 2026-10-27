//! `/foods`, each feeder's food over time, and the refills owners record.
//! Owners write them; everyone reads them.
//!
//! A feeder serves one food at a time. Setting another starts a new period
//! from that moment, so a past feed's calories use the food it served. A
//! portion is 1/12 cup (provisional), so a portion weighs
//! `grams_per_cup / 12`.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{FeederId, FoodId, RefillId, UserId};

/// The grams a cup holds when a food does not say: a typical dry kibble.
pub const GRAMS_PER_CUP: u32 = 110;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Food {
    pub id: FoodId,
    /// 1-64 characters.
    pub brand: String,
    /// 1-64 characters.
    pub name: String,
    /// Energy, as labels print it (1-10000 kcal per kg).
    pub kcal_per_kg: u32,
    /// A full bag's weight (1-100000 g): what a full-bag refill adds.
    pub bag_grams: u32,
    /// What a cup of it weighs (1-1000 g).
    pub grams_per_cup: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NewFood {
    pub brand: String,
    pub name: String,
    pub kcal_per_kg: u32,
    pub bag_grams: u32,
    /// 110 g when absent.
    #[serde(default = "grams_per_cup")]
    #[schema(default = 110)]
    pub grams_per_cup: u32,
}

fn grams_per_cup() -> u32 {
    GRAMS_PER_CUP
}

/// A change to a food; an absent field is left as it is. A change applies
/// to the past too: a new recipe is a new food.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FoodPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brand: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kcal_per_kg: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bag_grams: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grams_per_cup: Option<u32>,
}

/// A span of time a feeder served one food: from `from` until `to`, the
/// next change, or still when `to` is `None`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FoodPeriod {
    pub food: Food,
    pub from: Timestamp,
    pub to: Option<Timestamp>,
}

/// A feeder's food as it stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FeederFood {
    /// The food it serves now; `None` before an owner sets one.
    pub current: Option<FoodPeriod>,
    /// The newest refill recorded.
    pub last_refill: Option<Refill>,
}

/// `PUT /feeders/{id}/food`: the food a feeder serves from now on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FoodChoice {
    pub food: FoodId,
}

/// A page of a feeder's food history, oldest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FoodHistoryPage {
    pub items: Vec<FoodPeriod>,
    /// The `after` for the next page; `None` at the end.
    pub next: Option<String>,
}

/// What an owner put in the hopper.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NewRefill {
    /// A weighed amount (1-100000 g).
    Grams { grams: u32 },
    /// A whole bag of the feeder's current food: its `bag_grams`. A feeder
    /// with no food set is refused `no_current_food`.
    FullBag,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Refill {
    pub id: RefillId,
    pub feeder: FeederId,
    pub at: Timestamp,
    /// What went in: a full bag's is the bag size when it was recorded.
    pub grams: u32,
    pub full_bag: bool,
    /// The feeder's food when it was recorded.
    pub food: Option<FoodId>,
    /// Who recorded it.
    pub user: UserId,
}

/// A page of a feeder's refills, oldest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RefillPage {
    pub items: Vec<Refill>,
    /// The `after` for the next page; `None` at the end.
    pub next: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cup_is_110_grams_unless_the_food_says() {
        let food: NewFood =
            serde_json::from_str(r#"{"brand":"B","name":"N","kcal_per_kg":3600,"bag_grams":2000}"#)
                .unwrap();
        assert_eq!(food.grams_per_cup, 110);
    }

    #[test]
    fn a_refill_names_its_kind() {
        let parse = |json| serde_json::from_str::<NewRefill>(json).unwrap();

        assert_eq!(
            parse(r#"{"kind":"grams","grams":500}"#),
            NewRefill::Grams { grams: 500 }
        );
        assert_eq!(parse(r#"{"kind":"full_bag"}"#), NewRefill::FullBag);
        assert!(serde_json::from_str::<NewRefill>(r#"{"grams":500}"#).is_err());
    }
}

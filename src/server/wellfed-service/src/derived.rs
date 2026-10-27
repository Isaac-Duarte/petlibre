//! Derived views: what the owner's data and the feeders' histories add up
//! to. Pure functions of typed rows, with the time and the household's
//! zone passed in, so every rule here is tested without a store, a clock or
//! a feeder; `views.rs` reads the rows and calls them.
//!
//! - **A feed's time** is the feeder's own when its clock was valid (when
//!   the food dropped), else when the server received the result.
//! - **A feed's food** is the one the feeder served at that time, so a past
//!   feed keeps its food's weight and energy after the food changes.
//! - **A portion** is 1/12 cup (provisional), weighed by the food's
//!   `grams_per_cup`. Only the portions the feeder confirmed count.
//! - **Who ate** is not known: a feed from a feeder that served several
//!   pets at its time is split evenly between them.
//!
//! Weights are kept in milligrams and energy in thousandths of a kcal, in
//! integers, so a sum never depends on the order it was added in; each is
//! rounded once, for the answer.

mod food;
mod intake;
#[cfg(test)]
mod properties;
#[cfg(test)]
mod tests;

use jiff::Timestamp;
use wellfed_api::{Food, FoodPeriod, PetId, PetLink};
use wellfed_store::{Feed, Projected};

pub(crate) use food::{FoodInputs, food_view, hopper};
pub(crate) use intake::{Days, FeederHistory, intake};

/// Twelve portions to a cup, provisionally.
const PORTIONS_PER_CUP: u64 = 12;

/// When the feed happened: the feeder's time when its clock was valid,
/// else when the server received it.
pub(crate) fn feed_time(feed: &Projected<Feed>) -> Timestamp {
    feed.at.unwrap_or(feed.received)
}

/// What a portion of `food` weighs, in milligrams.
pub(crate) fn portion_mg(food: &Food) -> u64 {
    u64::from(food.grams_per_cup) * 1000 / PORTIONS_PER_CUP
}

/// The food the feeder served at `at`: the last change made by then, its
/// own instant included. `periods` are oldest first.
pub(crate) fn food_at(periods: &[FoodPeriod], at: Timestamp) -> Option<&Food> {
    periods
        .iter()
        .rev()
        .find(|period| period.from <= at)
        .map(|period| &period.food)
}

/// What a feed dispensed: its confirmed portions, and their weight and
/// energy by its food (`None` with no food set at its time).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Weighed {
    pub(crate) portions: u64,
    pub(crate) mg: Option<u64>,
    /// Thousandths of a kcal.
    pub(crate) mkcal: Option<u64>,
}

pub(crate) fn weigh(feed: &Projected<Feed>, periods: &[FoodPeriod]) -> Weighed {
    let portions = u64::from(feed.value.completed);
    let food = food_at(periods, feed_time(feed));
    let mg = food.map(|food| portions * portion_mg(food));
    // kcal/kg × mg is a millionth of a kcal; a thousandth is kept.
    let mkcal = food
        .zip(mg)
        .map(|(food, mg)| mg * u64::from(food.kcal_per_kg) / 1000);
    Weighed {
        portions,
        mg,
        mkcal,
    }
}

/// Whether `link` ran at `at`.
fn covers(link: &PetLink, at: Timestamp) -> bool {
    link.from <= at && link.to.is_none_or(|to| at < to)
}

/// The pets the feeder served at `at`, each once.
pub(crate) fn served_at(links: &[PetLink], at: Timestamp) -> Vec<PetId> {
    let mut pets: Vec<PetId> = links
        .iter()
        .filter(|link| covers(link, at))
        .map(|link| link.pet)
        .collect();
    pets.sort_unstable();
    pets.dedup();
    pets
}

/// Thousandths rounded to the nearest whole, saturating.
fn whole(thousandths: u64) -> u32 {
    u32::try_from(thousandths.saturating_add(500) / 1000).unwrap_or(u32::MAX)
}

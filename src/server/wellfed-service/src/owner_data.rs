//! What the household keeps that the feeders do not: its own settings,
//! the pets, the foods and refills, and the samples read raw or by the
//! hour.

mod foods;
mod household;
mod pets;
mod samples;

use std::sync::Arc;

use wellfed_api::ErrorCode;

/// What the owner data keeps between requests.
#[derive(Clone, Default)]
pub(crate) struct State {
    /// Household changes run one at a time, so the stored zone and the
    /// mirror's are always the last change's.
    pub(crate) household: Arc<tokio::sync::Mutex<()>>,
}

/// Pets, foods and refills: the owner data the feeders do not hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum OwnerDataError {
    #[error("that feeder does not serve the pet")]
    NotAssigned,
    #[error("a feeder's food history names the food: past feeds' calories use it")]
    FoodInUse,
    #[error("the feeder has no food set, so a full bag weighs nothing known: set its food first")]
    NoCurrentFood,
}

impl OwnerDataError {
    pub(crate) fn code(self) -> ErrorCode {
        match self {
            Self::NotAssigned => ErrorCode::NotFound,
            Self::FoodInUse => ErrorCode::FoodInUse,
            Self::NoCurrentFood => ErrorCode::NoCurrentFood,
        }
    }
}

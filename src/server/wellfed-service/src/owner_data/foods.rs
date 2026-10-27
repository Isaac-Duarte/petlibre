//! Foods, the food each feeder serves over time, and refills.
//!
//! A feeder serves one food at a time. Setting another starts a period
//! from now, so a past feed's calories use the food it served:
//! [`Service::feeder_food_at`] answers which. A change is never dated
//! before the one it follows, even when the clock steps back, so the
//! newest change is always the current food. A food in a feeder's history
//! cannot be deleted.

use jiff::Timestamp;
use wellfed_api::{
    FeederFood, FeederId, Food, FoodHistoryPage, FoodId, FoodPatch, FoodPeriod, NewFood, NewRefill,
    PageQuery, Refill, RefillId, RefillPage, User,
};
use wellfed_store::FoodSet;

use crate::page::{self, after_seq};
use crate::{Error, Invalid, OwnerDataError, Resource, Result, Service};

const KCAL_MAX: u32 = 10_000;
const BAG_MAX: u32 = 100_000;
const CUP_MAX: u32 = 1000;
const REFILL_MAX: u32 = 100_000;

impl Service {
    /// Every food, by brand and name.
    pub async fn foods(&self) -> Result<Vec<Food>> {
        Ok(self.shared.store.foods().await?)
    }

    pub async fn food(&self, id: FoodId) -> Result<Food> {
        let food = self.shared.store.food(id).await?;
        food.ok_or_else(|| Error::NotFound(Resource::Food))
    }

    pub async fn create_food(&self, request: NewFood) -> Result<Food> {
        let food = Food {
            id: FoodId(self.shared.new_id()?),
            brand: label(&request.brand)?.to_owned(),
            name: label(&request.name)?.to_owned(),
            kcal_per_kg: within(request.kcal_per_kg, KCAL_MAX, Invalid::Kcal)?,
            bag_grams: within(request.bag_grams, BAG_MAX, Invalid::BagSize)?,
            grams_per_cup: within(request.grams_per_cup, CUP_MAX, Invalid::GramsPerCup)?,
        };

        self.shared
            .store
            .insert_food(&food, self.shared.clock.now())
            .await?;
        Ok(food)
    }

    /// Change a food. Its history keeps naming it, so a change applies to
    /// past feeds too.
    pub async fn update_food(&self, id: FoodId, patch: FoodPatch) -> Result<Food> {
        let mut food = self.food(id).await?;
        if let Some(brand) = patch.brand {
            food.brand = label(&brand)?.to_owned();
        }
        if let Some(name) = patch.name {
            food.name = label(&name)?.to_owned();
        }
        if let Some(kcal) = patch.kcal_per_kg {
            food.kcal_per_kg = within(kcal, KCAL_MAX, Invalid::Kcal)?;
        }
        if let Some(bag) = patch.bag_grams {
            food.bag_grams = within(bag, BAG_MAX, Invalid::BagSize)?;
        }
        if let Some(cup) = patch.grams_per_cup {
            food.grams_per_cup = within(cup, CUP_MAX, Invalid::GramsPerCup)?;
        }

        if !self.shared.store.update_food(&food).await? {
            return Err(Error::NotFound(Resource::Food));
        }
        Ok(food)
    }

    /// Delete a food no feeder's history or refill names.
    pub async fn delete_food(&self, id: FoodId) -> Result<()> {
        match self.shared.store.delete_food(id).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(Error::NotFound(Resource::Food)),
            Err(wellfed_store::Error::InUse(_)) => Err(OwnerDataError::FoodInUse.into()),
            Err(error) => Err(error.into()),
        }
    }

    /// The feeder's food now (its newest change, even when the clock has
    /// stepped back before it), and its newest refill.
    pub async fn feeder_food(&self, feeder: FeederId) -> Result<FeederFood> {
        self.row(feeder).await?;

        let store = &self.shared.store;
        let current = store.current_food(feeder).await?;
        let last_refill = store.last_refill(feeder).await?;
        Ok(FeederFood {
            current,
            last_refill,
        })
    }

    /// The food the feeder served at `at`, and the period it served it in:
    /// the last change made by then, the one made exactly at `at`
    /// included. `None` before its first. A feed is asked at the feeder's
    /// own time when its clock was valid (when the food dropped), else at
    /// when the server received it.
    pub async fn feeder_food_at(
        &self,
        feeder: FeederId,
        at: Timestamp,
    ) -> Result<Option<FoodPeriod>> {
        self.row(feeder).await?;
        Ok(self.shared.store.feeder_food_at(feeder, at).await?)
    }

    /// Serve `food` from now on. The food the feeder already serves keeps
    /// its period.
    pub async fn set_feeder_food(&self, feeder: FeederId, food: FoodId) -> Result<FoodPeriod> {
        self.row(feeder).await?;
        let food = self.food(food).await?;

        let set = self
            .shared
            .store
            .set_feeder_food(feeder, food.id, self.shared.clock.now())
            .await?;
        let (FoodSet::Started(from) | FoodSet::Unchanged(from)) = set;
        Ok(FoodPeriod {
            food,
            from,
            to: None,
        })
    }

    /// A page of the feeder's food history, oldest first.
    pub async fn food_history(
        &self,
        feeder: FeederId,
        query: PageQuery,
    ) -> Result<FoodHistoryPage> {
        let limit = page::limit(&query)?;
        let after = after_seq(&query)?;
        self.row(feeder).await?;

        let rows = self
            .shared
            .store
            .food_history(feeder, after, limit + 1)
            .await?;
        let (rows, next) = page::split(rows, limit, |(seq, _)| seq.to_string());
        Ok(FoodHistoryPage {
            items: rows.into_iter().map(|(_, period)| period).collect(),
            next,
        })
    }

    /// Record what `user` put in the feeder's hopper now (never before its
    /// last refill, should the clock step back). A full bag is the current
    /// food's bag size.
    pub async fn add_refill(
        &self,
        user: &User,
        feeder: FeederId,
        request: NewRefill,
    ) -> Result<Refill> {
        if let NewRefill::Grams { grams } = request {
            within(grams, REFILL_MAX, Invalid::RefillGrams)?;
        }
        self.row(feeder).await?;

        let current = self.shared.store.current_food(feeder).await?;
        let grams = match (request, &current) {
            (NewRefill::Grams { grams }, _) => grams,
            (NewRefill::FullBag, Some(period)) => period.food.bag_grams,
            (NewRefill::FullBag, None) => return Err(OwnerDataError::NoCurrentFood.into()),
        };
        let mut refill = Refill {
            id: RefillId(self.shared.new_id()?),
            feeder,
            at: self.shared.clock.now(),
            grams,
            full_bag: request == NewRefill::FullBag,
            food: current.map(|period| period.food.id),
            user: user.id,
        };

        refill.at = self.shared.store.insert_refill(&refill).await?;
        Ok(refill)
    }

    /// A page of the feeder's refills, in the order they were recorded.
    pub async fn refills(&self, feeder: FeederId, query: PageQuery) -> Result<RefillPage> {
        let limit = page::limit(&query)?;
        let after = after_seq(&query)?;
        self.row(feeder).await?;

        let rows = self.shared.store.refills(feeder, after, limit + 1).await?;
        let (rows, next) = page::split(rows, limit, |(seq, _)| seq.to_string());
        Ok(RefillPage {
            items: rows.into_iter().map(|(_, refill)| refill).collect(),
            next,
        })
    }

    pub async fn delete_refill(&self, feeder: FeederId, id: RefillId) -> Result<()> {
        self.row(feeder).await?;

        if !self.shared.store.delete_refill(feeder, id).await? {
            return Err(Error::NotFound(Resource::Refill));
        }
        Ok(())
    }
}

/// A brand or a name: 1-64 characters, trimmed.
fn label(text: &str) -> Result<&str, Invalid> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > 64 {
        return Err(Invalid::FoodName);
    }
    Ok(text)
}

fn within(value: u32, max: u32, invalid: Invalid) -> Result<u32, Invalid> {
    if !(1..=max).contains(&value) {
        return Err(invalid);
    }
    Ok(value)
}

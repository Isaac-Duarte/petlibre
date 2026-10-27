//! The derived views' reads: the rows each needs, from the store, handed
//! to the pure functions in `derived`.

use jiff::civil::Date;
use jiff::{SignedDuration, Timestamp};
use wellfed_api::{
    FeederFood, FeederId, FoodPeriod, INTAKE_DAYS_MAX, IntakeQuery, PetId, PetIntake,
};

use crate::derived::{self, Days, FeederHistory, FoodInputs};
use crate::{Invalid, Result, Service};

/// How far back the food beam's reads are searched for an unrecorded
/// refill when no refill is recorded.
const REFILL_LOOKBACK: SignedDuration = SignedDuration::from_hours(30 * 24);

impl Service {
    /// The feeder's food now (its newest change, even when the clock has
    /// stepped back before it), its newest refill, the hopper as the beam
    /// last read it, and what is left since the refill.
    pub async fn feeder_food(&self, feeder: FeederId) -> Result<FeederFood> {
        self.row(feeder).await?;

        let store = &self.shared.store;
        let now = self.shared.clock.now();
        let current = store.current_food(feeder).await?;
        let last_refill = store.last_refill(feeder).await?;
        let since = now.checked_sub(REFILL_LOOKBACK).unwrap_or(now);
        let start = last_refill.as_ref().map_or(since, |refill| refill.at);
        // A refill's count starts at the refill: a feed is received after
        // it happened, so none of its feeds was received before it.
        let feeds = match last_refill {
            Some(_) => store.feeds(feeder, start, Timestamp::MAX).await?,
            None => Vec::new(),
        };
        let periods = self.food_periods(feeder).await?;
        let readings = store.food_readings(feeder, start, Timestamp::MAX).await?;
        let last = store.last_food_reading(feeder).await?;

        let (estimate, unrecorded_refill) = derived::food_view(&FoodInputs {
            current: current.as_ref(),
            last_refill: last_refill.as_ref(),
            feeds: &feeds,
            periods: &periods,
            readings: &readings,
            since,
            now,
        });
        Ok(FeederFood {
            current,
            last_refill,
            hopper: derived::hopper(last.as_ref()),
            estimate,
            unrecorded_refill,
        })
    }

    /// What the pet's feeders dispensed for it each day of `query`, in the
    /// household's zone, and the feeds among them that went wrong.
    pub async fn pet_intake(&self, pet: PetId, query: IntakeQuery) -> Result<PetIntake> {
        let zone = self.household_zone();
        let today = self.shared.clock.now().to_zoned(zone.clone()).date();
        let to = query.to.unwrap_or(today);
        let from = match query.from {
            Some(from) => from,
            None => to.checked_sub(jiff::Span::new().days(6)).unwrap_or(to),
        };
        check_days(from, to)?;
        let pet = self.pet(pet).await?;
        let days = Days { from, to, zone };
        let (start, end) = days.span().ok_or(Invalid::IntakeDays)?;

        let store = &self.shared.store;
        let mut feeders: Vec<FeederId> = store
            .pet_links(pet.id, start, end)
            .await?
            .into_iter()
            .map(|link| link.feeder)
            .collect();
        feeders.sort_unstable();
        feeders.dedup();

        let mut rows = Vec::with_capacity(feeders.len());
        for feeder in feeders {
            // A feed is received at or after it happened, so every feed of
            // the days was received since they began.
            let feeds = store.feeds(feeder, start, Timestamp::MAX).await?;
            let periods = self.food_periods(feeder).await?;
            let links = store.feeder_links(feeder, start, end).await?;
            rows.push((feeder, feeds, periods, links));
        }
        let histories: Vec<FeederHistory<'_>> = rows
            .iter()
            .map(|(feeder, feeds, periods, links)| FeederHistory {
                feeder: *feeder,
                feeds,
                periods,
                links,
            })
            .collect();

        Ok(derived::intake(pet.id, pet.daily_kcal, &histories, &days))
    }

    /// The feeder's whole food history, oldest first. A feeder's food
    /// changes rarely, so it is read whole.
    async fn food_periods(&self, feeder: FeederId) -> Result<Vec<FoodPeriod>> {
        let rows = self
            .shared
            .store
            .food_history(feeder, None, u32::MAX)
            .await?;
        Ok(rows.into_iter().map(|(_, period)| period).collect())
    }
}

/// `from` is on or before `to`, at most 366 days before it.
fn check_days(from: Date, to: Date) -> Result<(), Invalid> {
    let days = from
        .until(to)
        .ok()
        .map(|span| i64::from(span.get_days()))
        .ok_or(Invalid::IntakeDays)?;
    if !(0..i64::from(INTAKE_DAYS_MAX)).contains(&days) {
        return Err(Invalid::IntakeDays);
    }
    Ok(())
}

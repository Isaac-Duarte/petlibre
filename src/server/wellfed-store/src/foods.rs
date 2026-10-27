//! Foods, and the food each feeder served over time.
//!
//! A feeder's food history is a list of changes, each starting a period
//! that runs until the next. A change is never dated before the one it
//! follows, so the periods are in `seq` order by time too, and the food at
//! a time is the last change made by then.

use jiff::Timestamp;
use wellfed_api::{FeederId, Food, FoodId, FoodPeriod};
use uuid::Uuid;

use crate::convert::{from_i64, millis, timestamp, to_i64};
use crate::{Error, Result, Store};

/// Whether [`Store::set_feeder_food`] started a period.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoodSet {
    /// A period from this time.
    Started(Timestamp),
    /// The feeder already serves that food, since this time.
    Unchanged(Timestamp),
}

impl Store {
    /// Every food, by brand and name.
    pub async fn foods(&self) -> Result<Vec<Food>> {
        let rows = sqlx::query_as!(
            Raw,
            r#"SELECT id AS "id: Uuid", brand, name, kcal_per_kg, bag_grams, grams_per_cup
               FROM foods ORDER BY brand, name, id"#
        )
        .fetch_all(&self.reader)
        .await?;
        rows.into_iter().map(Food::try_from).collect()
    }

    pub async fn food(&self, id: FoodId) -> Result<Option<Food>> {
        let row = sqlx::query_as!(
            Raw,
            r#"SELECT id AS "id: Uuid", brand, name, kcal_per_kg, bag_grams, grams_per_cup
               FROM foods WHERE id = ?"#,
            id.0
        )
        .fetch_optional(&self.reader)
        .await?;
        row.map(Food::try_from).transpose()
    }

    pub async fn insert_food(&self, food: &Food, created: Timestamp) -> Result<()> {
        let created = millis(created);
        sqlx::query!(
            "INSERT INTO foods (id, brand, name, kcal_per_kg, bag_grams, grams_per_cup, created_ms)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            food.id.0,
            food.brand,
            food.name,
            food.kcal_per_kg,
            food.bag_grams,
            food.grams_per_cup,
            created,
        )
        .execute(&self.writer)
        .await?;
        Ok(())
    }

    /// Replace a food's fields; whether it exists.
    pub async fn update_food(&self, food: &Food) -> Result<bool> {
        let done = sqlx::query!(
            "UPDATE foods SET brand = ?, name = ?, kcal_per_kg = ?, bag_grams = ?, grams_per_cup = ?
             WHERE id = ?",
            food.brand,
            food.name,
            food.kcal_per_kg,
            food.bag_grams,
            food.grams_per_cup,
            food.id.0,
        )
        .execute(&self.writer)
        .await?;
        Ok(done.rows_affected() == 1)
    }

    /// Delete a food; whether it existed. One a feeder's history or a
    /// refill names is [`Error::InUse`].
    pub async fn delete_food(&self, id: FoodId) -> Result<bool> {
        let done = sqlx::query!("DELETE FROM foods WHERE id = ?", id.0)
            .execute(&self.writer)
            .await;
        match done {
            Ok(done) => Ok(done.rows_affected() == 1),
            Err(sqlx::Error::Database(error)) if error.is_foreign_key_violation() => {
                Err(Error::InUse("food"))
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Make `food` the feeder's from `now`, or from its last change when
    /// the clock reads earlier, so the newest change is always the current
    /// one. Setting the food it serves changes nothing.
    pub async fn set_feeder_food(
        &self,
        feeder: FeederId,
        food: FoodId,
        now: Timestamp,
    ) -> Result<FoodSet> {
        let mut tx = self.writer.begin().await?;
        let last = sqlx::query!(
            r#"SELECT food_id AS "food_id: Uuid", from_ms FROM feeder_foods
               WHERE feeder_id = ? ORDER BY seq DESC LIMIT 1"#,
            feeder.0
        )
        .fetch_optional(&mut *tx)
        .await?;

        if let Some(last) = &last
            && last.food_id == food.0
        {
            return Ok(FoodSet::Unchanged(timestamp(last.from_ms)?));
        }
        let from = last.map_or(millis(now), |last| last.from_ms.max(millis(now)));
        sqlx::query!(
            "INSERT INTO feeder_foods (feeder_id, food_id, from_ms) VALUES (?, ?, ?)",
            feeder.0,
            food.0,
            from,
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(FoodSet::Started(timestamp(from)?))
    }

    /// The feeder's food now: its newest change, whatever the clock reads.
    /// A clock that stepped back reads before that change, so asking
    /// [`Store::feeder_food_at`] for "now" would miss it.
    pub async fn current_food(&self, feeder: FeederId) -> Result<Option<FoodPeriod>> {
        let row = sqlx::query_as!(
            Period,
            r#"SELECT p.seq, p.from_ms, NULL AS "to_ms?: i64",
                      f.id AS "id: Uuid", f.brand, f.name, f.kcal_per_kg, f.bag_grams,
                      f.grams_per_cup
               FROM feeder_foods p JOIN foods f ON f.id = p.food_id
               WHERE p.feeder_id = ?
               ORDER BY p.seq DESC LIMIT 1"#,
            feeder.0,
        )
        .fetch_optional(&self.reader)
        .await?;
        row.map(|row| row.period().map(|(_, period)| period))
            .transpose()
    }

    /// The period the feeder's food at `at` belongs to: the last change
    /// made by then. `None` before its first.
    pub async fn feeder_food_at(
        &self,
        feeder: FeederId,
        at: Timestamp,
    ) -> Result<Option<FoodPeriod>> {
        let at = millis(at);
        let row = sqlx::query_as!(
            Period,
            r#"SELECT p.seq, p.from_ms,
                      (SELECT n.from_ms FROM feeder_foods n
                       WHERE n.feeder_id = p.feeder_id AND n.seq > p.seq
                       ORDER BY n.seq LIMIT 1) AS "to_ms?: i64",
                      f.id AS "id: Uuid", f.brand, f.name, f.kcal_per_kg, f.bag_grams,
                      f.grams_per_cup
               FROM feeder_foods p JOIN foods f ON f.id = p.food_id
               WHERE p.feeder_id = ? AND p.from_ms <= ?
               ORDER BY p.seq DESC LIMIT 1"#,
            feeder.0,
            at,
        )
        .fetch_optional(&self.reader)
        .await?;
        row.map(|row| row.period().map(|(_, period)| period))
            .transpose()
    }

    /// Up to `limit` of the feeder's periods after the one numbered
    /// `after`, oldest first, each with its number.
    pub async fn food_history(
        &self,
        feeder: FeederId,
        after: Option<u64>,
        limit: u32,
    ) -> Result<Vec<(u64, FoodPeriod)>> {
        let after = after.map_or(Ok(0), |after| to_i64(after, "cursor"))?;
        let rows = sqlx::query_as!(
            Period,
            r#"SELECT p.seq, p.from_ms,
                      (SELECT n.from_ms FROM feeder_foods n
                       WHERE n.feeder_id = p.feeder_id AND n.seq > p.seq
                       ORDER BY n.seq LIMIT 1) AS "to_ms?: i64",
                      f.id AS "id: Uuid", f.brand, f.name, f.kcal_per_kg, f.bag_grams,
                      f.grams_per_cup
               FROM feeder_foods p JOIN foods f ON f.id = p.food_id
               WHERE p.feeder_id = ? AND p.seq > ?
               ORDER BY p.seq LIMIT ?"#,
            feeder.0,
            after,
            limit,
        )
        .fetch_all(&self.reader)
        .await?;
        rows.into_iter().map(Period::period).collect()
    }
}

/// A food as SQLite holds it.
struct Raw {
    id: Uuid,
    brand: String,
    name: String,
    kcal_per_kg: i64,
    bag_grams: i64,
    grams_per_cup: i64,
}

impl TryFrom<Raw> for Food {
    type Error = Error;

    fn try_from(raw: Raw) -> Result<Self> {
        let small = |value: i64| u32::try_from(value).map_err(|_| Error::Corrupt("food"));
        Ok(Self {
            id: FoodId(raw.id),
            brand: raw.brand,
            name: raw.name,
            kcal_per_kg: small(raw.kcal_per_kg)?,
            bag_grams: small(raw.bag_grams)?,
            grams_per_cup: small(raw.grams_per_cup)?,
        })
    }
}

/// A period and its food, as SQLite holds them.
struct Period {
    seq: i64,
    from_ms: i64,
    to_ms: Option<i64>,
    id: Uuid,
    brand: String,
    name: String,
    kcal_per_kg: i64,
    bag_grams: i64,
    grams_per_cup: i64,
}

impl Period {
    fn period(self) -> Result<(u64, FoodPeriod)> {
        let seq = from_i64(self.seq, "food history")?;
        let from = timestamp(self.from_ms)?;
        let to = self.to_ms.map(timestamp).transpose()?;
        let food = Food::try_from(Raw {
            id: self.id,
            brand: self.brand,
            name: self.name,
            kcal_per_kg: self.kcal_per_kg,
            bag_grams: self.bag_grams,
            grams_per_cup: self.grams_per_cup,
        })?;
        Ok((seq, FoodPeriod { food, from, to }))
    }
}

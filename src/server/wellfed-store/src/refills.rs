//! The refills owners record: what went into each feeder's hopper, and
//! when.

use jiff::Timestamp;
use uuid::Uuid;
use wellfed_api::{FeederId, FoodId, Refill, RefillId, UserId};

use crate::convert::{from_i64, millis, timestamp, to_i64};
use crate::{Error, Result, Store};

impl Store {
    /// Record a refill at its time, or at the feeder's last refill's when
    /// the clock reads earlier, so refills in recording order are in time
    /// order too. The time as stored, to the millisecond.
    pub async fn insert_refill(&self, refill: &Refill) -> Result<Timestamp> {
        let at = millis(refill.at);
        let food = refill.food.map(|food| food.0);
        let stored = sqlx::query_scalar!(
            r#"INSERT INTO refills (id, feeder_id, at_ms, grams, full_bag, food_id, user_id)
               VALUES (?1, ?2, MAX(?3, COALESCE(
                   (SELECT MAX(at_ms) FROM refills WHERE feeder_id = ?2), ?3)),
                   ?4, ?5, ?6, ?7)
               RETURNING at_ms"#,
            refill.id.0,
            refill.feeder.0,
            at,
            refill.grams,
            refill.full_bag,
            food,
            refill.user.0,
        )
        .fetch_one(&self.writer)
        .await?;
        timestamp(stored)
    }

    /// Up to `limit` of the feeder's refills after the one numbered
    /// `after`, in the order they were recorded, each with its number.
    pub async fn refills(
        &self,
        feeder: FeederId,
        after: Option<u64>,
        limit: u32,
    ) -> Result<Vec<(u64, Refill)>> {
        let after = after.map_or(Ok(0), |after| to_i64(after, "cursor"))?;
        let rows = sqlx::query_as!(
            Raw,
            r#"SELECT seq, id AS "id: Uuid", feeder_id AS "feeder_id: Uuid", at_ms, grams,
                      full_bag AS "full_bag: bool", food_id AS "food_id: Uuid",
                      user_id AS "user_id: Uuid"
               FROM refills WHERE feeder_id = ? AND seq > ? ORDER BY seq LIMIT ?"#,
            feeder.0,
            after,
            limit,
        )
        .fetch_all(&self.reader)
        .await?;
        rows.into_iter().map(Raw::refill).collect()
    }

    /// The feeder's newest refill.
    pub async fn last_refill(&self, feeder: FeederId) -> Result<Option<Refill>> {
        let row = sqlx::query_as!(
            Raw,
            r#"SELECT seq, id AS "id: Uuid", feeder_id AS "feeder_id: Uuid", at_ms, grams,
                      full_bag AS "full_bag: bool", food_id AS "food_id: Uuid",
                      user_id AS "user_id: Uuid"
               FROM refills WHERE feeder_id = ? ORDER BY seq DESC LIMIT 1"#,
            feeder.0,
        )
        .fetch_optional(&self.reader)
        .await?;
        row.map(|row| row.refill().map(|(_, refill)| refill))
            .transpose()
    }

    /// Whether the feeder had that refill.
    pub async fn delete_refill(&self, feeder: FeederId, id: RefillId) -> Result<bool> {
        let done = sqlx::query!(
            "DELETE FROM refills WHERE id = ? AND feeder_id = ?",
            id.0,
            feeder.0,
        )
        .execute(&self.writer)
        .await?;
        Ok(done.rows_affected() == 1)
    }
}

/// A refill as SQLite holds it.
struct Raw {
    seq: i64,
    id: Uuid,
    feeder_id: Uuid,
    at_ms: i64,
    grams: i64,
    full_bag: bool,
    food_id: Option<Uuid>,
    user_id: Uuid,
}

impl Raw {
    fn refill(self) -> Result<(u64, Refill)> {
        let refill = Refill {
            id: RefillId(self.id),
            feeder: FeederId(self.feeder_id),
            at: timestamp(self.at_ms)?,
            grams: u32::try_from(self.grams).map_err(|_| Error::Corrupt("refill"))?,
            full_bag: self.full_bag,
            food: self.food_id.map(FoodId),
            user: UserId(self.user_id),
        };
        Ok((from_i64(self.seq, "refill")?, refill))
    }
}

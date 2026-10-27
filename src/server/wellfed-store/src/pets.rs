//! Pets, their weight logs and the feeders that serve them.

use std::collections::HashMap;

use jiff::Timestamp;
use jiff::civil::Date;
use wellfed_api::{FeederId, Pet, PetId, PetLink, Species, Weight, WeightId};
use uuid::Uuid;

use crate::convert::{millis, timestamp};
use crate::{Error, Result, Store};

/// A place in a weight log: its entries come after this reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WeightCursor {
    pub at: Timestamp,
    pub id: WeightId,
}

impl Store {
    /// Every pet, by name, each with its feeders.
    pub async fn pets(&self) -> Result<Vec<Pet>> {
        let rows = sqlx::query!(
            r#"SELECT id AS "id: Uuid", name, species, birth_date, daily_kcal
               FROM pets ORDER BY name, id"#
        )
        .fetch_all(&self.reader)
        .await?;
        let links = sqlx::query!(
            r#"SELECT pet_id AS "pet_id: Uuid", feeder_id AS "feeder_id: Uuid"
               FROM pet_feeders WHERE to_ms IS NULL ORDER BY feeder_id"#
        )
        .fetch_all(&self.reader)
        .await?;

        let mut feeders: HashMap<Uuid, Vec<FeederId>> = HashMap::new();
        for link in links {
            feeders
                .entry(link.pet_id)
                .or_default()
                .push(FeederId(link.feeder_id));
        }
        rows.into_iter()
            .map(|row| {
                let serves = feeders.remove(&row.id).unwrap_or_default();
                pet(
                    row.id,
                    row.name,
                    &row.species,
                    row.birth_date,
                    row.daily_kcal,
                    serves,
                )
            })
            .collect()
    }

    pub async fn pet(&self, id: PetId) -> Result<Option<Pet>> {
        let row = sqlx::query!(
            r#"SELECT id AS "id: Uuid", name, species, birth_date, daily_kcal
               FROM pets WHERE id = ?"#,
            id.0
        )
        .fetch_optional(&self.reader)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };

        let serves = sqlx::query!(
            r#"SELECT feeder_id AS "feeder_id: Uuid" FROM pet_feeders
               WHERE pet_id = ? AND to_ms IS NULL ORDER BY feeder_id"#,
            id.0
        )
        .fetch_all(&self.reader)
        .await?
        .into_iter()
        .map(|link| FeederId(link.feeder_id))
        .collect();
        pet(
            row.id,
            row.name,
            &row.species,
            row.birth_date,
            row.daily_kcal,
            serves,
        )
        .map(Some)
    }

    /// Add a pet; its feeders are assigned separately.
    pub async fn insert_pet(&self, pet: &Pet, created: Timestamp) -> Result<()> {
        let (species, birth, created) = (pet.species.code(), date(pet), millis(created));
        sqlx::query!(
            "INSERT INTO pets (id, name, species, birth_date, daily_kcal, created_ms)
             VALUES (?, ?, ?, ?, ?, ?)",
            pet.id.0,
            pet.name,
            species,
            birth,
            pet.daily_kcal,
            created,
        )
        .execute(&self.writer)
        .await?;
        Ok(())
    }

    /// Replace a pet's own fields (not its feeders); whether it exists.
    pub async fn update_pet(&self, pet: &Pet) -> Result<bool> {
        let (species, birth) = (pet.species.code(), date(pet));
        let done = sqlx::query!(
            "UPDATE pets SET name = ?, species = ?, birth_date = ?, daily_kcal = ? WHERE id = ?",
            pet.name,
            species,
            birth,
            pet.daily_kcal,
            pet.id.0,
        )
        .execute(&self.writer)
        .await?;
        Ok(done.rows_affected() == 1)
    }

    /// Delete a pet, its weight log and its feeders' links; whether it
    /// existed.
    pub async fn delete_pet(&self, id: PetId) -> Result<bool> {
        let done = sqlx::query!("DELETE FROM pets WHERE id = ?", id.0)
            .execute(&self.writer)
            .await?;
        Ok(done.rows_affected() == 1)
    }

    /// Let `feeder` serve `pet` from `now`, or from when their last link
    /// ended if the clock reads earlier, so a pair's links never overlap.
    /// A link that is open stays as it is.
    pub async fn assign(&self, pet: PetId, feeder: FeederId, now: Timestamp) -> Result<()> {
        let now = millis(now);
        sqlx::query!(
            "INSERT INTO pet_feeders (pet_id, feeder_id, from_ms)
             SELECT ?1, ?2, MAX(?3, COALESCE(
                 (SELECT MAX(to_ms) FROM pet_feeders WHERE pet_id = ?1 AND feeder_id = ?2),
                 ?3))
             WHERE NOT EXISTS (SELECT 1 FROM pet_feeders
                               WHERE pet_id = ?1 AND feeder_id = ?2 AND to_ms IS NULL)",
            pet.0,
            feeder.0,
            now,
        )
        .execute(&self.writer)
        .await?;
        Ok(())
    }

    /// End the pair's open link at `now` (never before it began); whether
    /// `feeder` served `pet` until then.
    pub async fn unassign(&self, pet: PetId, feeder: FeederId, now: Timestamp) -> Result<bool> {
        let now = millis(now);
        let done = sqlx::query!(
            "UPDATE pet_feeders SET to_ms = MAX(from_ms, ?)
             WHERE pet_id = ? AND feeder_id = ? AND to_ms IS NULL",
            now,
            pet.0,
            feeder.0,
        )
        .execute(&self.writer)
        .await?;
        Ok(done.rows_affected() == 1)
    }

    /// The feeder's links that overlap `from..to` (`to` exclusive), in the
    /// order they began. A link that began and ended at one instant served
    /// no time, so it overlaps nothing.
    pub async fn feeder_links(
        &self,
        feeder: FeederId,
        from: Timestamp,
        to: Timestamp,
    ) -> Result<Vec<PetLink>> {
        let (from, to) = (millis(from), millis(to));
        let rows = sqlx::query!(
            r#"SELECT pet_id AS "pet_id: Uuid", from_ms, to_ms FROM pet_feeders
               WHERE feeder_id = ?1 AND from_ms < ?3 AND (to_ms IS NULL OR to_ms > ?2)
                     AND (to_ms IS NULL OR to_ms > from_ms)
               ORDER BY from_ms, seq"#,
            feeder.0,
            from,
            to,
        )
        .fetch_all(&self.reader)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(PetLink {
                    pet: PetId(row.pet_id),
                    feeder,
                    from: timestamp(row.from_ms)?,
                    to: row.to_ms.map(timestamp).transpose()?,
                })
            })
            .collect()
    }

    /// The pet's links that overlap `from..to` (`to` exclusive), in the
    /// order they began, as [`Store::feeder_links`] reads a feeder's.
    pub async fn pet_links(
        &self,
        pet: PetId,
        from: Timestamp,
        to: Timestamp,
    ) -> Result<Vec<PetLink>> {
        let (from, to) = (millis(from), millis(to));
        let rows = sqlx::query!(
            r#"SELECT feeder_id AS "feeder_id: Uuid", from_ms, to_ms FROM pet_feeders
               WHERE pet_id = ?1 AND from_ms < ?3 AND (to_ms IS NULL OR to_ms > ?2)
                     AND (to_ms IS NULL OR to_ms > from_ms)
               ORDER BY from_ms, seq"#,
            pet.0,
            from,
            to,
        )
        .fetch_all(&self.reader)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(PetLink {
                    pet,
                    feeder: FeederId(row.feeder_id),
                    from: timestamp(row.from_ms)?,
                    to: row.to_ms.map(timestamp).transpose()?,
                })
            })
            .collect()
    }

    /// Log a weight; the reading time as stored, to the millisecond.
    pub async fn insert_weight(&self, pet: PetId, weight: &Weight) -> Result<Timestamp> {
        let at = millis(weight.at);
        sqlx::query!(
            "INSERT INTO pet_weights (id, pet_id, at_ms, grams) VALUES (?, ?, ?, ?)",
            weight.id.0,
            pet.0,
            at,
            weight.grams,
        )
        .execute(&self.writer)
        .await?;
        timestamp(at)
    }

    /// Up to `limit` of a pet's weights after `after`, oldest reading
    /// first.
    pub async fn weights(
        &self,
        pet: PetId,
        after: Option<WeightCursor>,
        limit: u32,
    ) -> Result<Vec<Weight>> {
        let at = after.map(|after| millis(after.at));
        let id = after.map(|after| after.id.0);
        let rows = sqlx::query!(
            r#"SELECT id AS "id: Uuid", at_ms, grams FROM pet_weights
               WHERE pet_id = ?1 AND (?2 IS NULL OR (at_ms, id) > (?2, ?3))
               ORDER BY at_ms, id LIMIT ?4"#,
            pet.0,
            at,
            id,
            limit,
        )
        .fetch_all(&self.reader)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(Weight {
                    id: WeightId(row.id),
                    at: timestamp(row.at_ms)?,
                    grams: u32::try_from(row.grams).map_err(|_| Error::Corrupt("weight"))?,
                })
            })
            .collect()
    }

    /// Whether the pet's log held that entry.
    pub async fn delete_weight(&self, pet: PetId, weight: WeightId) -> Result<bool> {
        let done = sqlx::query!(
            "DELETE FROM pet_weights WHERE id = ? AND pet_id = ?",
            weight.0,
            pet.0,
        )
        .execute(&self.writer)
        .await?;
        Ok(done.rows_affected() == 1)
    }
}

fn date(pet: &Pet) -> Option<String> {
    pet.birth_date.map(|date| date.to_string())
}

fn pet(
    id: Uuid,
    name: String,
    species: &str,
    birth_date: Option<String>,
    daily_kcal: Option<i64>,
    feeders: Vec<FeederId>,
) -> Result<Pet> {
    let species = Species::from_code(species).ok_or(Error::Corrupt("species"))?;
    let birth_date = birth_date
        .map(|text| {
            text.parse::<Date>()
                .map_err(|_| Error::Corrupt("birth date"))
        })
        .transpose()?;
    let daily_kcal = daily_kcal
        .map(|kcal| u32::try_from(kcal).map_err(|_| Error::Corrupt("daily kcal")))
        .transpose()?;
    Ok(Pet {
        id: PetId(id),
        name,
        species,
        birth_date,
        daily_kcal,
        feeders,
    })
}

//! Restoring an export onto a fresh database, in one transaction.
//!
//! An [`Import`] holds the writer for as long as it runs, so nothing else
//! writes meanwhile, and a setup racing it either finds the imported
//! owners or wins before the import begins and refuses it. Rows go in
//! exactly as exported, ids and sequence numbers included, so exporting
//! the result gives the same rows. Dropping an import unfinished rolls it
//! all back.

use jiff::Timestamp;
use sqlx::{Sqlite, Transaction};
use wellfed_formats::export::Row;

use crate::convert::millis;
use crate::{Result, Store};

/// An import in progress.
#[derive(Debug)]
pub struct Import {
    tx: Transaction<'static, Sqlite>,
    /// What each imported account's password hash is set to.
    password_hash: String,
}

impl Store {
    /// Begin an import, or `None` when any account exists: only a fresh
    /// server takes one. Each imported account gets `password_hash`.
    pub async fn begin_import(&self, password_hash: &str) -> Result<Option<Import>> {
        let mut tx = self.writer.begin().await?;
        let taken = sqlx::query!(r#"SELECT EXISTS (SELECT 1 FROM users) AS "taken!: bool""#)
            .fetch_one(&mut *tx)
            .await?;
        if taken.taken {
            return Ok(None);
        }
        Ok(Some(Import {
            tx,
            password_hash: password_hash.to_owned(),
        }))
    }
}

impl Import {
    /// Insert one row. Rows must come in the archive's table order, so
    /// each row's foreign keys name rows already in.
    pub async fn insert(&mut self, row: &Row) -> Result<()> {
        let tx = &mut *self.tx;
        match row {
            Row::Users(user) => {
                sqlx::query!(
                    "INSERT INTO users (id, name, password_hash, role, created_ms, expires_ms)
                     VALUES (?, ?, ?, ?, ?, ?)",
                    user.id,
                    user.name,
                    self.password_hash,
                    user.role,
                    user.created_ms,
                    user.expires_ms,
                )
                .execute(tx)
                .await?;
            }
            Row::Household(household) => {
                sqlx::query!(
                    "INSERT INTO household (id, name, units, time_format, zone)
                     VALUES (1, ?, ?, ?, ?)",
                    household.name,
                    household.units,
                    household.time_format,
                    household.zone,
                )
                .execute(tx)
                .await?;
            }
            Row::Feeders(feeder) => {
                // No key, only a nonce of the stored width: the feeder is
                // archived, and adopting it again stores its new key.
                sqlx::query!(
                    "INSERT INTO feeders (id, identity, name, role, address, key_nonce, key_sealed,
                                          synced, epoch, created_ms)
                     VALUES (?, ?, ?, ?, ?, zeroblob(12), X'', ?, ?, ?)",
                    feeder.id,
                    feeder.identity,
                    feeder.name,
                    feeder.role,
                    feeder.address,
                    feeder.synced,
                    feeder.epoch,
                    feeder.created_ms,
                )
                .execute(tx)
                .await?;
            }
            Row::FeederState(state) => {
                sqlx::query!(
                    "INSERT INTO feeder_state (feeder_id, status, status_ms, settings, settings_ms)
                     VALUES (?, ?, ?, ?, ?)",
                    state.feeder_id,
                    state.status,
                    state.status_ms,
                    state.settings,
                    state.settings_ms,
                )
                .execute(tx)
                .await?;
            }
            Row::PhoneKeys(phone) => rows::phone_key(tx, phone).await?,
            Row::KeptClients(kept) => rows::kept_client(tx, kept).await?,
            Row::Events(event) => {
                sqlx::query!(
                    "INSERT INTO events (feeder_id, epoch, seq, at_s, received_ms, kind, body)
                     VALUES (?, ?, ?, ?, ?, ?, ?)",
                    event.feeder_id,
                    event.epoch,
                    event.seq,
                    event.at_s,
                    event.received_ms,
                    event.kind,
                    event.body,
                )
                .execute(tx)
                .await?;
            }
            Row::Gaps(gap) => {
                sqlx::query!(
                    "INSERT INTO gaps (feeder_id, epoch, from_seq, to_seq, received_ms)
                     VALUES (?, ?, ?, ?, ?)",
                    gap.feeder_id,
                    gap.epoch,
                    gap.from_seq,
                    gap.to_seq,
                    gap.received_ms,
                )
                .execute(tx)
                .await?;
            }
            Row::Samples(sample) => {
                // The id is kept: AUTOINCREMENT's sequence follows it up, so
                // no later sample reuses one, and the rollups' mark holds.
                sqlx::query!(
                    "INSERT INTO samples (id, feeder_id, received_ms, uptime_ms, metric, number,
                                          value)
                     VALUES (?, ?, ?, ?, ?, ?, ?)",
                    sample.id,
                    sample.feeder_id,
                    sample.received_ms,
                    sample.uptime_ms,
                    sample.metric,
                    sample.number,
                    sample.value,
                )
                .execute(tx)
                .await?;
            }
            Row::SampleRollups(rollup) => {
                sqlx::query!(
                    "INSERT INTO sample_rollups (feeder_id, metric, hour_ms, count, min, max, sum)
                     VALUES (?, ?, ?, ?, ?, ?, ?)",
                    rollup.feeder_id,
                    rollup.metric,
                    rollup.hour_ms,
                    rollup.count,
                    rollup.min,
                    rollup.max,
                    rollup.sum,
                )
                .execute(tx)
                .await?;
            }
            Row::SampleMarks(marks) => {
                // The migration made the one row.
                sqlx::query!(
                    "UPDATE sample_marks SET rolled = ?, version = ? WHERE id = 0",
                    marks.rolled,
                    marks.version,
                )
                .execute(tx)
                .await?;
            }
            Row::ProjectionVersion(version) => {
                // A fresh server's start may have written its own.
                sqlx::query!(
                    "INSERT INTO projection_version (id, version) VALUES (0, ?)
                     ON CONFLICT (id) DO UPDATE SET version = excluded.version",
                    version.version,
                )
                .execute(tx)
                .await?;
            }
            Row::Feeds(feed) => rows::feed(tx, feed).await?,
            Row::LockChanges(change) => rows::lock_change(tx, change).await?,
            Row::Boots(boot) => rows::boot(tx, boot).await?,
            Row::ClockSyncs(sync) => rows::clock_sync(tx, sync).await?,
            Row::BlockedChanges(change) => rows::blocked_change(tx, change).await?,
            Row::FeedTraces(trace) => rows::feed_trace(tx, trace).await?,
            Row::FeederStats(stat) => rows::feeder_stat(tx, stat).await?,
            Row::FoodReadings(reading) => rows::food_reading(tx, reading).await?,
            Row::Commands(command) => rows::command(tx, command).await?,
            Row::Audit(entry) => rows::audit(tx, entry).await?,
            Row::Installs(install) => rows::install(tx, install).await?,
            Row::Pets(pet) => rows::pet(tx, pet).await?,
            Row::PetWeights(weight) => rows::pet_weight(tx, weight).await?,
            Row::PetFeeders(link) => rows::pet_feeder(tx, link).await?,
            Row::Foods(food) => rows::food(tx, food).await?,
            Row::FeederFoods(food) => rows::feeder_food(tx, food).await?,
            Row::Refills(refill) => rows::refill(tx, refill).await?,
        }
        Ok(())
    }

    /// Archive every imported feeder as of `at` (the archive holds no key
    /// for any of them) and make the import whole. How many feeders it
    /// archived.
    pub async fn commit(mut self, at: Timestamp) -> Result<u64> {
        let at = millis(at);
        let archived = sqlx::query!("UPDATE feeders SET archived_ms = ?", at)
            .execute(&mut *self.tx)
            .await?;

        self.tx.commit().await?;
        Ok(archived.rows_affected())
    }
}

mod rows;

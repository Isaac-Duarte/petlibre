//! Reading every exported table out of one snapshot, a row at a time.
//!
//! A [`Snapshot`] is a read transaction on a reader connection: in WAL
//! mode it sees the database as of its first read for as long as it is
//! held, while ingest and every other write go on. Each table comes out in
//! its primary key's order, so the same database always gives the same
//! rows in the same order, however often it is read.

use futures_util::stream::BoxStream;
use futures_util::{StreamExt, TryStreamExt};
use sqlx::{Sqlite, Transaction};
use uuid::Uuid;
use wellfed_formats::export::{
    AuditEntry, BlockedChange, Boot, ClockSync, Command, Event, Feed, FeedTrace, Feeder,
    FeederFood, FeederStat, FeederState, Food, FoodReading, Gap, Household, Install, KeptClient,
    LockChange, Pet, PetFeeder, PetWeight, PhoneKey, ProjectionVersion, Refill, Row, Sample,
    SampleMarks, SampleRollup, Table, User,
};

use crate::{Error, Result, Store};

/// One consistent view of the database, read table by table.
#[derive(Debug)]
pub struct Snapshot(Transaction<'static, Sqlite>);

impl Store {
    /// Open a snapshot. It holds one reader until it is dropped.
    pub async fn snapshot(&self) -> Result<Snapshot> {
        Ok(Snapshot(self.reader.begin().await?))
    }
}

impl Snapshot {
    /// `table`'s rows in its primary key's order, read as they are taken.
    pub fn rows(&mut self, table: Table) -> BoxStream<'_, Result<Row>> {
        let tx = &mut *self.0;
        match table {
            Table::Users => rows(
                sqlx::query_as!(
                    User,
                    r#"SELECT id AS "id: Uuid", name, role, created_ms, expires_ms
                       FROM users ORDER BY id"#
                )
                .fetch(tx),
            ),
            Table::Household => rows(
                sqlx::query_as!(
                    Household,
                    "SELECT name, units, time_format, zone FROM household ORDER BY id"
                )
                .fetch(tx),
            ),
            Table::Feeders => rows(
                sqlx::query_as!(
                    Feeder,
                    r#"SELECT id AS "id: Uuid", identity, name, role, address, synced, epoch,
                              created_ms
                       FROM feeders ORDER BY id"#
                )
                .fetch(tx),
            ),
            Table::FeederState => rows(
                sqlx::query_as!(
                    FeederState,
                    r#"SELECT feeder_id AS "feeder_id: Uuid", status, status_ms, settings,
                              settings_ms, plans, plans_ms
                       FROM feeder_state ORDER BY feeder_id"#
                )
                .fetch(tx),
            ),
            Table::PhoneKeys => rows(
                sqlx::query_as!(
                    PhoneKey,
                    r#"SELECT feeder_id AS "feeder_id: Uuid", key, user_id AS "user_id: Uuid",
                              token_id AS "token_id: Uuid", role, invited_ms, invite_until_ms,
                              revoke, revoke_ms, attempt
                       FROM phone_keys ORDER BY feeder_id, key"#
                )
                .fetch(tx),
            ),
            Table::KeptClients => rows(
                sqlx::query_as!(
                    KeptClient,
                    r#"SELECT feeder_id AS "feeder_id: Uuid", key, kept_ms
                       FROM kept_clients ORDER BY feeder_id, key"#
                )
                .fetch(tx),
            ),
            Table::Events => rows(
                sqlx::query_as!(
                    Event,
                    r#"SELECT feeder_id AS "feeder_id: Uuid", epoch, seq, at_s, received_ms, kind,
                              body
                       FROM events ORDER BY feeder_id, epoch, seq"#
                )
                .fetch(tx),
            ),
            Table::Gaps => rows(
                sqlx::query_as!(
                    Gap,
                    r#"SELECT feeder_id AS "feeder_id: Uuid", epoch, from_seq, to_seq, received_ms
                       FROM gaps ORDER BY feeder_id, epoch, from_seq"#
                )
                .fetch(tx),
            ),
            Table::Samples => rows(
                sqlx::query_as!(
                    Sample,
                    r#"SELECT id AS "id!", feeder_id AS "feeder_id: Uuid", received_ms, uptime_ms,
                              metric, number, value
                       FROM samples ORDER BY id"#
                )
                .fetch(tx),
            ),
            Table::SampleRollups => rows(
                sqlx::query_as!(
                    SampleRollup,
                    r#"SELECT feeder_id AS "feeder_id: Uuid", metric, hour_ms, count, min, max, sum
                       FROM sample_rollups ORDER BY feeder_id, metric, hour_ms"#
                )
                .fetch(tx),
            ),
            Table::SampleMarks => rows(
                sqlx::query_as!(
                    SampleMarks,
                    "SELECT rolled, version FROM sample_marks ORDER BY id"
                )
                .fetch(tx),
            ),
            Table::ProjectionVersion => rows(
                sqlx::query_as!(
                    ProjectionVersion,
                    "SELECT version FROM projection_version ORDER BY id"
                )
                .fetch(tx),
            ),
            Table::Feeds => rows(
                sqlx::query_as!(
                    Feed,
                    r#"SELECT feeder_id AS "feeder_id: Uuid", epoch, feed, seq, source, client,
                              requested, completed, outcome, reason, at_s, received_ms
                       FROM feeds ORDER BY feeder_id, epoch, feed"#
                )
                .fetch(tx),
            ),
            Table::LockChanges => rows(
                sqlx::query_as!(
                    LockChange,
                    r#"SELECT feeder_id AS "feeder_id: Uuid", epoch, seq, locked AS "locked: bool",
                              source, client, at_s, received_ms
                       FROM lock_changes ORDER BY feeder_id, epoch, seq"#
                )
                .fetch(tx),
            ),
            Table::Boots => rows(
                sqlx::query_as!(
                    Boot,
                    r#"SELECT feeder_id AS "feeder_id: Uuid", epoch, seq, bank, reset_flags, at_s,
                              received_ms
                       FROM boots ORDER BY feeder_id, epoch, seq"#
                )
                .fetch(tx),
            ),
            Table::ClockSyncs => rows(
                sqlx::query_as!(
                    ClockSync,
                    r#"SELECT feeder_id AS "feeder_id: Uuid", epoch, seq, source, client, jump_ms,
                              at_s, received_ms
                       FROM clock_syncs ORDER BY feeder_id, epoch, seq"#
                )
                .fetch(tx),
            ),
            Table::BlockedChanges => rows(
                sqlx::query_as!(
                    BlockedChange,
                    r#"SELECT feeder_id AS "feeder_id: Uuid", epoch, seq, reason, at_s, received_ms
                       FROM blocked_changes ORDER BY feeder_id, epoch, seq"#
                )
                .fetch(tx),
            ),
            Table::FeedTraces => rows(
                sqlx::query_as!(
                    FeedTrace,
                    r#"SELECT feeder_id AS "feeder_id: Uuid", epoch, feed, fetched_ms, summary,
                              stored AS "stored: bool", body, refusal
                       FROM feed_traces ORDER BY feeder_id, epoch, feed"#
                )
                .fetch(tx),
            ),
            Table::FeederStats => rows(
                sqlx::query_as!(
                    FeederStat,
                    r#"SELECT feeder_id AS "feeder_id: Uuid", section, received_ms, boot_count, stats
                       FROM feeder_stats ORDER BY feeder_id, section"#
                )
                .fetch(tx),
            ),
            // No key: in the order the rows went in, which an import keeps.
            Table::FoodReadings => rows(
                sqlx::query_as!(
                    FoodReading,
                    r#"SELECT feeder_id AS "feeder_id: Uuid", received_ms, boot_count,
                              present AS "present: bool", emptied, refilled, feeds_since_refill
                       FROM food_readings ORDER BY rowid"#
                )
                .fetch(tx),
            ),
            Table::Commands => rows(
                sqlx::query_as!(
                    Command,
                    r#"SELECT seq AS "seq!", key, user_id AS "user_id: Uuid",
                              token_id AS "token_id: Uuid", feeder_id AS "feeder_id: Uuid", kind,
                              body_hash, request_id, boot_count, state, outcome, feed, sent_ms,
                              updated_ms, history_epoch, history_seq
                       FROM commands ORDER BY seq"#
                )
                .fetch(tx),
            ),
            Table::Audit => rows(
                sqlx::query_as!(
                    AuditEntry,
                    r#"SELECT seq AS "seq!", at_ms, user_id AS "user_id: Uuid", user_name,
                              token_id AS "token_id: Uuid", action, command_seq
                       FROM audit ORDER BY seq"#
                )
                .fetch(tx),
            ),
            Table::Installs => rows(
                sqlx::query_as!(
                    Install,
                    r#"SELECT id AS "id: Uuid", feeder_id AS "feeder_id: Uuid", release_id,
                              version, user_id AS "user_id: Uuid", started_ms, state, written,
                              image_len, failure, finished_ms, target
                       FROM installs ORDER BY id"#
                )
                .fetch(tx),
            ),
            Table::Pets => rows(
                sqlx::query_as!(
                    Pet,
                    r#"SELECT id AS "id: Uuid", name, species, birth_date, daily_kcal, created_ms
                       FROM pets ORDER BY id"#
                )
                .fetch(tx),
            ),
            Table::PetWeights => rows(
                sqlx::query_as!(
                    PetWeight,
                    r#"SELECT id AS "id: Uuid", pet_id AS "pet_id: Uuid", at_ms, grams
                       FROM pet_weights ORDER BY id"#
                )
                .fetch(tx),
            ),
            Table::PetFeeders => rows(
                sqlx::query_as!(
                    PetFeeder,
                    r#"SELECT seq AS "seq!", pet_id AS "pet_id: Uuid", feeder_id AS "feeder_id: Uuid",
                              from_ms, to_ms
                       FROM pet_feeders ORDER BY seq"#
                )
                .fetch(tx),
            ),
            Table::Foods => rows(
                sqlx::query_as!(
                    Food,
                    r#"SELECT id AS "id: Uuid", brand, name, kcal_per_kg, bag_grams, grams_per_cup,
                              created_ms
                       FROM foods ORDER BY id"#
                )
                .fetch(tx),
            ),
            Table::FeederFoods => rows(
                sqlx::query_as!(
                    FeederFood,
                    r#"SELECT seq AS "seq!", feeder_id AS "feeder_id: Uuid",
                              food_id AS "food_id: Uuid", from_ms
                       FROM feeder_foods ORDER BY seq"#
                )
                .fetch(tx),
            ),
            Table::Refills => rows(
                sqlx::query_as!(
                    Refill,
                    r#"SELECT seq AS "seq!", id AS "id: Uuid", feeder_id AS "feeder_id: Uuid", at_ms,
                              grams, full_bag AS "full_bag: bool", food_id AS "food_id: Uuid",
                              user_id AS "user_id: Uuid"
                       FROM refills ORDER BY seq"#
                )
                .fetch(tx),
            ),
        }
    }
}

fn rows<'a, T: Into<Row> + Send + 'a>(
    stream: BoxStream<'a, sqlx::Result<T>>,
) -> BoxStream<'a, Result<Row>> {
    stream.map_ok(Into::into).map_err(Error::from).boxed()
}

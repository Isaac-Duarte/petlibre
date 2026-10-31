//! The inserts of the tables after the samples: the projections, the
//! journal and its audit log, installs and the owner data. Each row goes in
//! as exported.

use sqlx::SqliteConnection;
use wellfed_formats::export::{
    AuditEntry, BlockedChange, Boot, ClockSync, Command, Feed, FeedTrace, FeederFood, FeederStat,
    Food, FoodReading, Install, KeptClient, LockChange, Pet, PetFeeder, PetWeight, PhoneKey,
    Refill,
};

use crate::Result;

pub(super) async fn phone_key(tx: &mut SqliteConnection, phone: &PhoneKey) -> Result<()> {
    sqlx::query!(
        "INSERT INTO phone_keys (feeder_id, key, user_id, token_id, role, invited_ms,
                                 invite_until_ms, revoke, revoke_ms, attempt)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        phone.feeder_id,
        phone.key,
        phone.user_id,
        phone.token_id,
        phone.role,
        phone.invited_ms,
        phone.invite_until_ms,
        phone.revoke,
        phone.revoke_ms,
        phone.attempt,
    )
    .execute(tx)
    .await?;
    Ok(())
}

pub(super) async fn kept_client(tx: &mut SqliteConnection, kept: &KeptClient) -> Result<()> {
    sqlx::query!(
        "INSERT INTO kept_clients (feeder_id, key, kept_ms) VALUES (?, ?, ?)",
        kept.feeder_id,
        kept.key,
        kept.kept_ms,
    )
    .execute(tx)
    .await?;
    Ok(())
}

pub(super) async fn feed(tx: &mut SqliteConnection, feed: &Feed) -> Result<()> {
    sqlx::query!(
        "INSERT INTO feeds (feeder_id, epoch, feed, seq, source, client, requested, completed,
                            outcome, reason, at_s, received_ms)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        feed.feeder_id,
        feed.epoch,
        feed.feed,
        feed.seq,
        feed.source,
        feed.client,
        feed.requested,
        feed.completed,
        feed.outcome,
        feed.reason,
        feed.at_s,
        feed.received_ms,
    )
    .execute(tx)
    .await?;
    Ok(())
}

pub(super) async fn lock_change(tx: &mut SqliteConnection, change: &LockChange) -> Result<()> {
    sqlx::query!(
        "INSERT INTO lock_changes (feeder_id, epoch, seq, locked, source, client, at_s,
                                   received_ms)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        change.feeder_id,
        change.epoch,
        change.seq,
        change.locked,
        change.source,
        change.client,
        change.at_s,
        change.received_ms,
    )
    .execute(tx)
    .await?;
    Ok(())
}

pub(super) async fn boot(tx: &mut SqliteConnection, boot: &Boot) -> Result<()> {
    sqlx::query!(
        "INSERT INTO boots (feeder_id, epoch, seq, bank, reset_flags, at_s, received_ms)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        boot.feeder_id,
        boot.epoch,
        boot.seq,
        boot.bank,
        boot.reset_flags,
        boot.at_s,
        boot.received_ms,
    )
    .execute(tx)
    .await?;
    Ok(())
}

pub(super) async fn clock_sync(tx: &mut SqliteConnection, sync: &ClockSync) -> Result<()> {
    sqlx::query!(
        "INSERT INTO clock_syncs (feeder_id, epoch, seq, source, client, jump_ms, at_s,
                                  received_ms)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        sync.feeder_id,
        sync.epoch,
        sync.seq,
        sync.source,
        sync.client,
        sync.jump_ms,
        sync.at_s,
        sync.received_ms,
    )
    .execute(tx)
    .await?;
    Ok(())
}

pub(super) async fn blocked_change(
    tx: &mut SqliteConnection,
    change: &BlockedChange,
) -> Result<()> {
    sqlx::query!(
        "INSERT INTO blocked_changes (feeder_id, epoch, seq, reason, at_s, received_ms)
         VALUES (?, ?, ?, ?, ?, ?)",
        change.feeder_id,
        change.epoch,
        change.seq,
        change.reason,
        change.at_s,
        change.received_ms,
    )
    .execute(tx)
    .await?;
    Ok(())
}

pub(super) async fn feed_trace(tx: &mut SqliteConnection, trace: &FeedTrace) -> Result<()> {
    sqlx::query!(
        "INSERT INTO feed_traces (feeder_id, epoch, feed, fetched_ms, summary, stored, body,
                                  refusal)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        trace.feeder_id,
        trace.epoch,
        trace.feed,
        trace.fetched_ms,
        trace.summary,
        trace.stored,
        trace.body,
        trace.refusal,
    )
    .execute(tx)
    .await?;
    Ok(())
}

pub(super) async fn feeder_stat(tx: &mut SqliteConnection, stat: &FeederStat) -> Result<()> {
    sqlx::query!(
        "INSERT INTO feeder_stats (feeder_id, section, received_ms, boot_count, stats)
         VALUES (?, ?, ?, ?, ?)",
        stat.feeder_id,
        stat.section,
        stat.received_ms,
        stat.boot_count,
        stat.stats,
    )
    .execute(tx)
    .await?;
    Ok(())
}

pub(super) async fn food_reading(tx: &mut SqliteConnection, reading: &FoodReading) -> Result<()> {
    sqlx::query!(
        "INSERT INTO food_readings (feeder_id, received_ms, boot_count, present, ran_low,
                                    refilled, feeds_since_refill)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        reading.feeder_id,
        reading.received_ms,
        reading.boot_count,
        reading.present,
        reading.ran_low,
        reading.refilled,
        reading.feeds_since_refill,
    )
    .execute(tx)
    .await?;
    Ok(())
}

pub(super) async fn command(tx: &mut SqliteConnection, command: &Command) -> Result<()> {
    sqlx::query!(
        "INSERT INTO commands (seq, key, user_id, token_id, feeder_id, kind, body_hash,
                               request_id, boot_count, state, outcome, feed, sent_ms, updated_ms,
                               history_epoch, history_seq)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        command.seq,
        command.key,
        command.user_id,
        command.token_id,
        command.feeder_id,
        command.kind,
        command.body_hash,
        command.request_id,
        command.boot_count,
        command.state,
        command.outcome,
        command.feed,
        command.sent_ms,
        command.updated_ms,
        command.history_epoch,
        command.history_seq,
    )
    .execute(&mut *tx)
    .await?;

    // The journal's trigger has just added the entry's audit row; the
    // archive's audit table brings that row as it was, so this one goes.
    sqlx::query!("DELETE FROM audit WHERE command_seq = ?", command.seq)
        .execute(tx)
        .await?;
    Ok(())
}

pub(super) async fn audit(tx: &mut SqliteConnection, entry: &AuditEntry) -> Result<()> {
    sqlx::query!(
        "INSERT INTO audit (seq, at_ms, user_id, user_name, token_id, action, command_seq)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        entry.seq,
        entry.at_ms,
        entry.user_id,
        entry.user_name,
        entry.token_id,
        entry.action,
        entry.command_seq,
    )
    .execute(tx)
    .await?;
    Ok(())
}

pub(super) async fn install(tx: &mut SqliteConnection, install: &Install) -> Result<()> {
    sqlx::query!(
        "INSERT INTO installs (id, feeder_id, release_id, version, user_id, started_ms, state,
                               written, image_len, failure, finished_ms, target)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        install.id,
        install.feeder_id,
        install.release_id,
        install.version,
        install.user_id,
        install.started_ms,
        install.state,
        install.written,
        install.image_len,
        install.failure,
        install.finished_ms,
        install.target,
    )
    .execute(tx)
    .await?;
    Ok(())
}

pub(super) async fn pet(tx: &mut SqliteConnection, pet: &Pet) -> Result<()> {
    sqlx::query!(
        "INSERT INTO pets (id, name, species, birth_date, daily_kcal, created_ms)
         VALUES (?, ?, ?, ?, ?, ?)",
        pet.id,
        pet.name,
        pet.species,
        pet.birth_date,
        pet.daily_kcal,
        pet.created_ms,
    )
    .execute(tx)
    .await?;
    Ok(())
}

pub(super) async fn pet_weight(tx: &mut SqliteConnection, weight: &PetWeight) -> Result<()> {
    sqlx::query!(
        "INSERT INTO pet_weights (id, pet_id, at_ms, grams) VALUES (?, ?, ?, ?)",
        weight.id,
        weight.pet_id,
        weight.at_ms,
        weight.grams,
    )
    .execute(tx)
    .await?;
    Ok(())
}

pub(super) async fn pet_feeder(tx: &mut SqliteConnection, link: &PetFeeder) -> Result<()> {
    sqlx::query!(
        "INSERT INTO pet_feeders (seq, pet_id, feeder_id, from_ms, to_ms) VALUES (?, ?, ?, ?, ?)",
        link.seq,
        link.pet_id,
        link.feeder_id,
        link.from_ms,
        link.to_ms,
    )
    .execute(tx)
    .await?;
    Ok(())
}

pub(super) async fn food(tx: &mut SqliteConnection, food: &Food) -> Result<()> {
    sqlx::query!(
        "INSERT INTO foods (id, brand, name, kcal_per_kg, bag_grams, grams_per_cup, created_ms)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        food.id,
        food.brand,
        food.name,
        food.kcal_per_kg,
        food.bag_grams,
        food.grams_per_cup,
        food.created_ms,
    )
    .execute(tx)
    .await?;
    Ok(())
}

pub(super) async fn feeder_food(tx: &mut SqliteConnection, food: &FeederFood) -> Result<()> {
    sqlx::query!(
        "INSERT INTO feeder_foods (seq, feeder_id, food_id, from_ms) VALUES (?, ?, ?, ?)",
        food.seq,
        food.feeder_id,
        food.food_id,
        food.from_ms,
    )
    .execute(tx)
    .await?;
    Ok(())
}

pub(super) async fn refill(tx: &mut SqliteConnection, refill: &Refill) -> Result<()> {
    sqlx::query!(
        "INSERT INTO refills (seq, id, feeder_id, at_ms, grams, full_bag, food_id, user_id)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        refill.seq,
        refill.id,
        refill.feeder_id,
        refill.at_ms,
        refill.grams,
        refill.full_bag,
        refill.food_id,
        refill.user_id,
    )
    .execute(tx)
    .await?;
    Ok(())
}

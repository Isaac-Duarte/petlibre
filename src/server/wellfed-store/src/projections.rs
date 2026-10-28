//! Projections: typed rows decoded from the raw events, so SQL can filter,
//! count and join what the events say (feeds and their outcomes, the
//! panel lock, boots, clock syncs, feeding blocked and free again).
//!
//! The raw `events` table stays the truth. Ingest writes an event's
//! projection in the raw insert's transaction, only when the raw row is
//! new, so a replay changes neither. When the projection code changes,
//! [`PROJECTION_VERSION`] moves and [`Store::refresh_projections`] rebuilds
//! every projection from the raw rows at the next start. The store keeps
//! no decoder of its own (the bodies are the feeder's encoding, which the
//! mirror owns), so the caller passes one.

mod read;

pub(crate) use read::FeedRow;

use jiff::Timestamp;
use sqlx::SqliteConnection;
use uuid::Uuid;
use wellfed_api::{Blocked, EventBody, FeedOutcome, FeedReason, FeederId, Source};

use crate::convert::source_columns;
use crate::{Error, Result, Store};

/// The version of the projection code and tables. Bump it whenever what an
/// event projects to changes, so every database rebuilds its projections
/// from the raw events.
pub const PROJECTION_VERSION: u32 = 1;

/// Raw events read per query while rebuilding.
const BATCH: i64 = 512;

/// One projected event: what it says, and where it sits in the history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Projected<T> {
    pub epoch: u32,
    pub seq: u64,
    /// The feeder's clock when it happened; `None` while that was invalid.
    pub at: Option<Timestamp>,
    pub received: Timestamp,
    pub value: T,
}

/// A feed's result. The feeder numbers its feeds within an epoch, and
/// the row is that feed's latest event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Feed {
    pub feed: u32,
    pub by: Source,
    pub requested: u8,
    /// The portions the feedback sensor confirmed.
    pub completed: u8,
    pub outcome: FeedOutcome,
    pub reason: FeedReason,
}

/// The panel lock turned on or off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LockChange {
    pub locked: bool,
    pub by: Source,
}

/// A boot, with the bank it ran from and the reset flags behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Boot {
    pub bank: u8,
    pub reset_flags: u32,
}

/// The feeder's clock was set, and is valid from then on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockSync {
    pub by: Source,
    /// How far the clock moved, when it was valid before.
    pub jump_ms: Option<i32>,
}

/// What [`Store::refresh_projections`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refresh {
    /// The projections were built by this version already.
    Current,
    /// Every projection was rebuilt from `events` raw rows, of which
    /// `undecoded` did not decode and project nothing.
    Rebuilt { events: u64, undecoded: u64 },
}

/// Where an event sits, in its stored form.
#[derive(Clone, Copy)]
pub(crate) struct Place {
    pub(crate) epoch: u32,
    pub(crate) seq: i64,
    pub(crate) at_s: Option<i64>,
    pub(crate) received_ms: i64,
}

impl Store {
    /// Rebuild every projection from the raw events when they were built
    /// by another [`PROJECTION_VERSION`] (or never), with `decode` turning
    /// a stored body back into the event. One transaction on the writer,
    /// so readers see the old projections until the new ones are whole,
    /// and ingest waits for it. Run it at start, before ingest begins.
    pub async fn refresh_projections(
        &self,
        decode: impl Fn(&[u8]) -> Option<EventBody>,
    ) -> Result<Refresh> {
        let mut tx = self.writer.begin().await?;
        let stored = sqlx::query_scalar!("SELECT version FROM projection_version WHERE id = 0")
            .fetch_optional(&mut *tx)
            .await?;
        if stored == Some(i64::from(PROJECTION_VERSION)) {
            return Ok(Refresh::Current);
        }

        sqlx::query!("DELETE FROM feeds").execute(&mut *tx).await?;
        sqlx::query!("DELETE FROM lock_changes")
            .execute(&mut *tx)
            .await?;
        sqlx::query!("DELETE FROM boots").execute(&mut *tx).await?;
        sqlx::query!("DELETE FROM clock_syncs")
            .execute(&mut *tx)
            .await?;
        sqlx::query!("DELETE FROM blocked_changes")
            .execute(&mut *tx)
            .await?;

        let (mut events, mut undecoded) = (0, 0);
        let mut after: Option<(Vec<u8>, i64, i64)> = None;
        loop {
            let (feeder, epoch, seq) = after.map_or((None, 0, 0), |(f, e, s)| (Some(f), e, s));
            let rows = sqlx::query!(
                r#"SELECT feeder_id, epoch, seq, at_s, received_ms, body FROM events
                   WHERE ?1 IS NULL OR (feeder_id, epoch, seq) > (?1, ?2, ?3)
                   ORDER BY feeder_id, epoch, seq LIMIT ?4"#,
                feeder,
                epoch,
                seq,
                BATCH,
            )
            .fetch_all(&mut *tx)
            .await?;
            let Some(last) = rows.last() else {
                break;
            };
            after = Some((last.feeder_id.clone(), last.epoch, last.seq));

            for row in rows {
                events += 1;
                let Some(body) = decode(&row.body) else {
                    undecoded += 1;
                    continue;
                };
                let id =
                    Uuid::from_slice(&row.feeder_id).map_err(|_| Error::Corrupt("feeder id"))?;
                let place = Place {
                    epoch: u32::try_from(row.epoch).map_err(|_| Error::Corrupt("event epoch"))?,
                    seq: row.seq,
                    at_s: row.at_s,
                    received_ms: row.received_ms,
                };
                project(&mut tx, FeederId(id), place, body).await?;
            }
        }

        sqlx::query!(
            "INSERT INTO projection_version (id, version) VALUES (0, ?)
             ON CONFLICT (id) DO UPDATE SET version = excluded.version",
            PROJECTION_VERSION,
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Refresh::Rebuilt { events, undecoded })
    }
}

/// Write what one new event projects to. Events that carry nothing a view
/// reads project nothing; each is named, so a new kind of event needs a
/// decision here.
pub(crate) async fn project(
    conn: &mut SqliteConnection,
    id: FeederId,
    place: Place,
    body: EventBody,
) -> Result<()> {
    let Place {
        epoch,
        seq,
        at_s,
        received_ms,
    } = place;

    match body {
        EventBody::Fed {
            feed,
            requested,
            completed,
            outcome,
            reason,
            by,
        } => {
            let (source, client) = source_columns(by);
            let client = client.as_deref();
            let (outcome, reason) = (outcome.code(), reason.code());
            // A feed's later event replaces its row; an earlier one
            // arriving after it (a rebuild runs in order, live ingest
            // need not) leaves it, so the row never depends on the order.
            sqlx::query!(
                "INSERT INTO feeds (feeder_id, epoch, feed, seq, source, client, requested,
                                    completed, outcome, reason, at_s, received_ms)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT (feeder_id, epoch, feed) DO UPDATE SET
                     seq = excluded.seq, source = excluded.source, client = excluded.client,
                     requested = excluded.requested, completed = excluded.completed,
                     outcome = excluded.outcome, reason = excluded.reason,
                     at_s = excluded.at_s, received_ms = excluded.received_ms
                 WHERE excluded.seq > feeds.seq",
                id.0,
                epoch,
                feed,
                seq,
                source,
                client,
                requested,
                completed,
                outcome,
                reason,
                at_s,
                received_ms,
            )
            .execute(&mut *conn)
            .await?;
        }

        EventBody::Lock { locked, by } => {
            let (source, client) = source_columns(by);
            let client = client.as_deref();
            sqlx::query!(
                "INSERT INTO lock_changes (feeder_id, epoch, seq, locked, source, client, at_s,
                                           received_ms)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
                id.0,
                epoch,
                seq,
                locked,
                source,
                client,
                at_s,
                received_ms,
            )
            .execute(&mut *conn)
            .await?;
        }

        EventBody::Boot { bank, reset_flags } => {
            sqlx::query!(
                "INSERT INTO boots (feeder_id, epoch, seq, bank, reset_flags, at_s, received_ms)
                 VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
                id.0,
                epoch,
                seq,
                bank,
                reset_flags,
                at_s,
                received_ms,
            )
            .execute(&mut *conn)
            .await?;
        }

        EventBody::ClockSync { by, jump_ms } => {
            let (source, client) = source_columns(by);
            let client = client.as_deref();
            sqlx::query!(
                "INSERT INTO clock_syncs (feeder_id, epoch, seq, source, client, jump_ms, at_s,
                                          received_ms)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
                id.0,
                epoch,
                seq,
                source,
                client,
                jump_ms,
                at_s,
                received_ms,
            )
            .execute(&mut *conn)
            .await?;
        }

        EventBody::FeedingBlocked { reason } => {
            let reason = reason.map(Blocked::code);
            sqlx::query!(
                "INSERT INTO blocked_changes (feeder_id, epoch, seq, reason, at_s, received_ms)
                 VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
                id.0,
                epoch,
                seq,
                reason,
                at_s,
                received_ms,
            )
            .execute(&mut *conn)
            .await?;
        }

        // Settings, plans and Wi-Fi changes, meal reminders, missed meals,
        // refused Dispense presses, enrolments and revocations, lost events
        // and supply changes: nothing a view reads yet. A view that needs
        // one adds its table and bumps the version, and the rebuild fills it
        // from the history already held.
        EventBody::Settings { .. }
        | EventBody::Plans { .. }
        | EventBody::Missed { .. }
        | EventBody::MealSoon { .. }
        | EventBody::DispenseRefused { .. }
        | EventBody::Wifi { .. }
        | EventBody::Enrolled { .. }
        | EventBody::Revoked { .. }
        | EventBody::Lost { .. }
        | EventBody::Supply { .. } => {}
    }
    Ok(())
}

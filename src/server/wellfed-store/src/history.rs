//! The mirrored event history: events and gaps, keyed by feeder, ring
//! epoch and sequence, with the feeder's cursor moved in the same
//! transaction as each insert so a reconnect never skips or repeats one.

use jiff::Timestamp;
use wellfed_api::{EventBody, EventKind, FeederId, Gap};

use crate::convert::{from_i64, millis, seconds, timestamp, to_i64};
use crate::projections::{Place, project};
use crate::{Error, Result, Store};

pub struct NewEvent<'a> {
    /// The feeder's ring epoch the number counts in.
    pub epoch: u32,
    pub seq: u64,
    pub at: Option<Timestamp>,
    pub received: Timestamp,
    /// The event, decoded once, for its kind and its projection.
    pub body: EventBody,
    /// The body as the feeder encoded it (postcard bytes): what is stored.
    pub raw: &'a [u8],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventRow {
    pub seq: u64,
    pub at: Option<Timestamp>,
    pub received: Timestamp,
    pub kind: EventKind,
    pub body: Vec<u8>,
}

impl Store {
    /// Record one event, its projection and the feeder's cursor past it,
    /// together. An event already held changes nothing; whether it was
    /// new. An event of another epoch than the cursor's moves the cursor
    /// to it.
    pub async fn record_event(&self, id: FeederId, event: NewEvent<'_>) -> Result<bool> {
        let epoch = event.epoch;
        let seq = to_i64(event.seq, "event sequence")?;
        let at = event.at.map(|at| at.as_second());
        let received = millis(event.received);
        let kind = event.body.kind().code();

        let mut tx = self.writer.begin().await?;
        let done = sqlx::query!(
            "INSERT INTO events (feeder_id, epoch, seq, at_s, received_ms, kind, body)
             VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
            id.0,
            epoch,
            seq,
            at,
            received,
            kind,
            event.raw,
        )
        .execute(&mut *tx)
        .await?;
        let new = done.rows_affected() == 1;
        if new {
            let place = Place {
                epoch,
                seq,
                at_s: at,
                received_ms: received,
            };
            project(&mut tx, id, place, event.body).await?;
        }
        advance(&mut tx, id, epoch, seq).await?;
        tx.commit().await?;
        Ok(new)
    }

    /// Record that `from..=to` are lost, and move the cursor past them.
    pub async fn record_gap(
        &self,
        id: FeederId,
        epoch: u32,
        from: u64,
        to: u64,
        received: Timestamp,
    ) -> Result<()> {
        let from = to_i64(from, "gap start")?;
        let to = to_i64(to, "gap end")?;
        let received = millis(received);

        let mut tx = self.writer.begin().await?;
        sqlx::query!(
            "INSERT INTO gaps (feeder_id, epoch, from_seq, to_seq, received_ms) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT DO NOTHING",
            id.0,
            epoch,
            from,
            to,
            received,
        )
        .execute(&mut *tx)
        .await?;
        advance(&mut tx, id, epoch, to).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Up to `limit` events of `epoch` after `after` (from the oldest when
    /// `None`), oldest first, of `kind` only when given.
    pub async fn events(
        &self,
        id: FeederId,
        epoch: u32,
        after: Option<u64>,
        limit: u32,
        kind: Option<EventKind>,
    ) -> Result<Vec<EventRow>> {
        let after = after.map_or(Ok(-1), |after| to_i64(after, "cursor"))?;
        let kind = kind.map(EventKind::code);
        let rows = sqlx::query!(
            r#"SELECT seq, at_s, received_ms, kind, body FROM events
               WHERE feeder_id = ?1 AND epoch = ?2 AND seq > ?3 AND (?4 IS NULL OR kind = ?4)
               ORDER BY seq LIMIT ?5"#,
            id.0,
            epoch,
            after,
            kind,
            limit,
        )
        .fetch_all(&self.reader)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(EventRow {
                    seq: from_i64(row.seq, "event sequence")?,
                    at: seconds(row.at_s)?,
                    received: timestamp(row.received_ms)?,
                    kind: EventKind::from_code(&row.kind).ok_or(Error::Corrupt("event kind"))?,
                    body: row.body,
                })
            })
            .collect()
    }

    /// The gaps of `epoch` that end after `after` and at or before
    /// `through`.
    pub async fn gaps(
        &self,
        id: FeederId,
        epoch: u32,
        after: Option<u64>,
        through: u64,
    ) -> Result<Vec<Gap>> {
        let after = after.map_or(Ok(-1), |after| to_i64(after, "cursor"))?;
        let through = to_i64(through, "cursor")?;
        let rows = sqlx::query!(
            "SELECT from_seq, to_seq, received_ms FROM gaps
             WHERE feeder_id = ? AND epoch = ? AND to_seq > ? AND to_seq <= ? ORDER BY from_seq",
            id.0,
            epoch,
            after,
            through,
        )
        .fetch_all(&self.reader)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(Gap {
                    epoch,
                    from: from_i64(row.from_seq, "gap start")?,
                    to: from_i64(row.to_seq, "gap end")?,
                    received: timestamp(row.received_ms)?,
                })
            })
            .collect()
    }
}

/// Move the cursor up to `seq`, never back within one epoch; a new epoch
/// starts the cursor again.
async fn advance(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    id: FeederId,
    epoch: u32,
    seq: i64,
) -> Result<()> {
    sqlx::query!(
        "UPDATE feeders
         SET synced = CASE WHEN epoch IS ?1 THEN MAX(COALESCE(synced, -1), ?2) ELSE ?2 END,
             epoch = ?1
         WHERE id = ?3",
        epoch,
        seq,
        id.0
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

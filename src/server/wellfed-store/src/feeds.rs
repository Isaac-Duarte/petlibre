//! Feeds in the command journal: the entries still waiting on the
//! feeder's history, their resolution from it, and the feeds projection
//! that history is read from.

use jiff::Timestamp;
use wellfed_api::{CommandKind, CommandOutcome, CommandState, FeederId};
use uuid::Uuid;

use crate::commands::Raw;
use crate::convert::{millis, to_i64};
use crate::projections::FeedRow;
use crate::{CommandRow, Error, Feed, Projected, Result, Store};

impl Store {
    /// A feeder's feeds the history may still settle, oldest first: those
    /// in flight or unknown, those answered with their number, and those
    /// resolved `unrecorded`, which a `Fed` event found later replaces.
    pub async fn open_feeds(&self, feeder: FeederId) -> Result<Vec<CommandRow>> {
        let kind = CommandKind::Feed.code();
        let (sent, answered, unknown, resolved) = (
            CommandState::Sent.code(),
            CommandState::Answered.code(),
            CommandState::Unknown.code(),
            CommandState::Resolved.code(),
        );
        let unrecorded = outcome_json(&CommandOutcome::Unrecorded)?;
        let rows = sqlx::query_as!(
            Raw,
            r#"SELECT seq, key, user_id AS "user_id: Uuid", token_id AS "token_id: Uuid",
                      feeder_id AS "feeder_id: Uuid", kind, body_hash, request_id, boot_count,
                      history_epoch, history_seq, state, outcome, feed, sent_ms, updated_ms
               FROM commands
               WHERE feeder_id = ?1 AND kind = ?2
                 AND (state IN (?3, ?5) OR (state = ?4 AND feed IS NOT NULL)
                      OR (state = ?6 AND outcome = ?7))
               ORDER BY seq"#,
            feeder.0,
            kind,
            sent,
            answered,
            unknown,
            resolved,
            unrecorded,
        )
        .fetch_all(&self.reader)
        .await?;

        rows.into_iter().map(CommandRow::try_from).collect()
    }

    /// A feeder's feeds resolved `unrecorded`, oldest first: sent through
    /// the server, and absent from the history after a restart.
    pub async fn unrecorded_feeds(&self, feeder: FeederId) -> Result<Vec<CommandRow>> {
        let kind = CommandKind::Feed.code();
        let resolved = CommandState::Resolved.code();
        let unrecorded = outcome_json(&CommandOutcome::Unrecorded)?;
        let rows = sqlx::query_as!(
            Raw,
            r#"SELECT seq, key, user_id AS "user_id: Uuid", token_id AS "token_id: Uuid",
                      feeder_id AS "feeder_id: Uuid", kind, body_hash, request_id, boot_count,
                      history_epoch, history_seq, state, outcome, feed, sent_ms, updated_ms
               FROM commands
               WHERE feeder_id = ?1 AND kind = ?2 AND state = ?3 AND outcome = ?4
               ORDER BY seq"#,
            feeder.0,
            kind,
            resolved,
            unrecorded,
        )
        .fetch_all(&self.reader)
        .await?;

        rows.into_iter().map(CommandRow::try_from).collect()
    }

    /// Whether a feed entry of `feeder` already holds the feed number
    /// `feed` of `epoch`'s history: numbers count again in a ring that a
    /// storage wipe re-created, so one sent in another epoch is another
    /// feed.
    pub async fn feed_claimed(&self, feeder: FeederId, epoch: u32, feed: u32) -> Result<bool> {
        let kind = CommandKind::Feed.code();
        let found = sqlx::query_scalar!(
            r#"SELECT EXISTS (
                   SELECT 1 FROM commands
                   WHERE feeder_id = ?1 AND kind = ?2 AND feed = ?3
                     AND (history_epoch IS NULL OR history_epoch = ?4)
               ) AS "found: bool""#,
            feeder.0,
            kind,
            feed,
            epoch,
        )
        .fetch_one(&self.reader)
        .await?;
        Ok(found)
    }

    /// The feed numbered `feed` in the history of `epoch`, as its latest
    /// event projects it, once the server holds its `Fed` event.
    pub async fn feed_result(
        &self,
        feeder: FeederId,
        epoch: u32,
        feed: u32,
    ) -> Result<Option<Projected<Feed>>> {
        let row = sqlx::query_as!(
            FeedRow,
            r#"SELECT epoch AS "epoch!", feed AS "feed!", seq AS "seq!", source, client, requested,
                      completed, outcome, reason, at_s, received_ms
               FROM feeds WHERE feeder_id = ? AND epoch = ? AND feed = ?"#,
            feeder.0,
            epoch,
            feed,
        )
        .fetch_optional(&self.reader)
        .await?;

        row.map(Projected::try_from).transpose()
    }

    /// The feeds whose `Fed` event comes after event `after` of `epoch`,
    /// in history order.
    pub async fn feeds_after(
        &self,
        feeder: FeederId,
        epoch: u32,
        after: u64,
    ) -> Result<Vec<Projected<Feed>>> {
        let after = to_i64(after, "event sequence")?;
        let rows = sqlx::query_as!(
            FeedRow,
            r#"SELECT epoch AS "epoch!", feed AS "feed!", seq AS "seq!", source, client, requested,
                      completed, outcome, reason, at_s, received_ms
               FROM feeds WHERE feeder_id = ? AND epoch = ? AND seq > ? ORDER BY seq"#,
            feeder.0,
            epoch,
            after,
        )
        .fetch_all(&self.reader)
        .await?;

        rows.into_iter().map(Projected::try_from).collect()
    }

    /// Settle a feed from the feeder's history. Only an unknown entry, an
    /// answered one with its number (not a refusal), or one resolved
    /// `unrecorded` when the history now holds its `Fed` event is settled;
    /// never one in flight, nor a `Fed` resolution. Whether the entry
    /// changed.
    pub async fn resolve_command(
        &self,
        key: &[u8; 16],
        outcome: &CommandOutcome,
        at: Timestamp,
    ) -> Result<bool> {
        let key = key.as_slice();
        let json = outcome_json(outcome)?;
        let unrecorded = outcome_json(&CommandOutcome::Unrecorded)?;
        let replaces = matches!(outcome, CommandOutcome::Fed { .. });
        let feed = outcome.feed();
        let (answered, unknown, resolved) = (
            CommandState::Answered.code(),
            CommandState::Unknown.code(),
            CommandState::Resolved.code(),
        );
        let at = millis(at);
        let done = sqlx::query!(
            "UPDATE commands SET state = ?1, outcome = ?2, feed = COALESCE(?3, feed), updated_ms = ?4
             WHERE key = ?5
               AND (state = ?7 OR (state = ?6 AND feed IS NOT NULL)
                    OR (?8 AND state = ?1 AND outcome = ?9))",
            resolved,
            json,
            feed,
            at,
            key,
            answered,
            unknown,
            replaces,
            unrecorded,
        )
        .execute(&self.writer)
        .await?;
        Ok(done.rows_affected() == 1)
    }
}

fn outcome_json(outcome: &CommandOutcome) -> Result<String> {
    serde_json::to_string(outcome).map_err(|e| Error::Json("outcome", e))
}

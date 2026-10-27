//! The command journal: each write the server sends a feeder, recorded
//! before it is sent, then moved to its answer or to unknown. A row is
//! deleted only while nothing of it can have reached the feeder.

use jiff::Timestamp;
use wellfed_api::{CommandKind, CommandOutcome, CommandState, FeederId, UserId};
use uuid::Uuid;

use crate::convert::{from_i64, millis, timestamp, to_i64};
use crate::{Error, Result, Store};

/// A write about to be sent.
pub struct NewCommand {
    pub key: [u8; 16],
    pub user: UserId,
    pub feeder: FeederId,
    pub kind: CommandKind,
    /// SHA-256 of the request's canonical JSON.
    pub body_hash: [u8; 32],
    /// The device request id it goes out under.
    pub request_id: [u8; 16],
    pub boot_count: Option<u32>,
    pub sent: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandRow {
    /// Its place in the journal: the list's order and cursor.
    pub seq: u64,
    pub key: [u8; 16],
    pub user: UserId,
    pub feeder: FeederId,
    pub kind: CommandKind,
    pub body_hash: [u8; 32],
    pub request_id: [u8; 16],
    pub boot_count: Option<u32>,
    pub state: CommandState,
    pub outcome: Option<CommandOutcome>,
    pub feed: Option<u32>,
    pub sent: Timestamp,
    pub updated: Timestamp,
}

/// Whether [`Store::journal`] recorded the write, or found its key taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Journaled {
    New,
    /// The key's entry, as it stands: a retry, or a key reused.
    Existing(Box<CommandRow>),
}

/// Which entries [`Store::commands`] lists.
#[derive(Clone, Copy, Debug)]
pub struct CommandFilter {
    pub user: Option<UserId>,
    pub feeder: Option<FeederId>,
    pub state: Option<CommandState>,
    /// Entries after this `seq`; from the first when `None`.
    pub after: Option<u64>,
    pub limit: u32,
}

impl Store {
    /// Record `command` as sent, unless its key is already in the
    /// journal: then the entry it names, read in the same transaction, so
    /// two requests racing with one key see one entry.
    pub async fn journal(&self, command: &NewCommand) -> Result<Journaled> {
        let key = command.key.as_slice();
        let kind = command.kind.code();
        let hash = command.body_hash.as_slice();
        let request = command.request_id.as_slice();
        let state = CommandState::Sent.code();
        let sent = millis(command.sent);

        let mut tx = self.writer.begin().await?;
        let done = sqlx::query!(
            "INSERT INTO commands
                 (key, user_id, feeder_id, kind, body_hash, request_id, boot_count, state, sent_ms, updated_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9) ON CONFLICT (key) DO NOTHING",
            key,
            command.user.0,
            command.feeder.0,
            kind,
            hash,
            request,
            command.boot_count,
            state,
            sent,
        )
        .execute(&mut *tx)
        .await?;
        if done.rows_affected() == 1 {
            tx.commit().await?;
            return Ok(Journaled::New);
        }

        let found = sqlx::query_as!(
            Raw,
            r#"SELECT seq, key, user_id AS "user_id: Uuid", feeder_id AS "feeder_id: Uuid", kind,
                      body_hash, request_id, boot_count, state, outcome, feed, sent_ms, updated_ms
               FROM commands WHERE key = ?"#,
            key
        )
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Journaled::Existing(Box::new(found.try_into()?)))
    }

    /// Record the feeder's answer. An entry already resolved from the
    /// history keeps its resolution. Whether an entry changed: `false`
    /// when there is none under `key`, or it is resolved.
    pub async fn answer_command(
        &self,
        key: &[u8; 16],
        outcome: &CommandOutcome,
        at: Timestamp,
    ) -> Result<bool> {
        let key = key.as_slice();
        let json = serde_json::to_string(outcome).map_err(|e| Error::Json("outcome", e))?;
        let (answered, resolved) = (CommandState::Answered.code(), CommandState::Resolved.code());
        let at = millis(at);
        let done = sqlx::query!(
            "UPDATE commands SET state = ?1, outcome = ?2, updated_ms = ?3
             WHERE key = ?4 AND state != ?5",
            answered,
            json,
            at,
            key,
            resolved,
        )
        .execute(&self.writer)
        .await?;
        Ok(done.rows_affected() == 1)
    }

    /// No answer came for a write that may have reached the feeder. An
    /// entry another attempt already answered stays answered. Whether an
    /// entry changed: `false` when there is none under `key`, or it is
    /// answered or resolved.
    pub async fn command_unknown(&self, key: &[u8; 16], at: Timestamp) -> Result<bool> {
        let key = key.as_slice();
        let (sent, unknown) = (CommandState::Sent.code(), CommandState::Unknown.code());
        let at = millis(at);
        let done = sqlx::query!(
            "UPDATE commands SET state = ?1, updated_ms = ?2 WHERE key = ?3 AND state IN (?4, ?1)",
            unknown,
            at,
            key,
            sent,
        )
        .execute(&self.writer)
        .await?;
        Ok(done.rows_affected() == 1)
    }

    /// Drop the entry of a write that was never sent (the feeder was
    /// offline), so its key is free again. Only an entry still `sent` is
    /// dropped: one another attempt moved on is kept.
    pub async fn forget_unsent(&self, key: &[u8; 16]) -> Result<()> {
        let key = key.as_slice();
        let sent = CommandState::Sent.code();
        sqlx::query!(
            "DELETE FROM commands WHERE key = ? AND state = ?",
            key,
            sent
        )
        .execute(&self.writer)
        .await?;
        Ok(())
    }

    /// Mark every write still `sent` unknown. The server runs this as it
    /// starts: whoever sent them stopped before an answer was recorded,
    /// and the feeder may have run them. How many there were.
    pub async fn interrupt_commands(&self, at: Timestamp) -> Result<u64> {
        let (sent, unknown) = (CommandState::Sent.code(), CommandState::Unknown.code());
        let at = millis(at);
        let done = sqlx::query!(
            "UPDATE commands SET state = ?, updated_ms = ? WHERE state = ?",
            unknown,
            at,
            sent,
        )
        .execute(&self.writer)
        .await?;
        Ok(done.rows_affected())
    }

    pub async fn command(&self, key: &[u8; 16]) -> Result<Option<CommandRow>> {
        let key = key.as_slice();
        let found = sqlx::query_as!(
            Raw,
            r#"SELECT seq, key, user_id AS "user_id: Uuid", feeder_id AS "feeder_id: Uuid", kind,
                      body_hash, request_id, boot_count, state, outcome, feed, sent_ms, updated_ms
               FROM commands WHERE key = ?"#,
            key
        )
        .fetch_optional(&self.reader)
        .await?;

        found.map(CommandRow::try_from).transpose()
    }

    /// Up to `filter.limit` entries after `filter.after`, oldest first.
    pub async fn commands(&self, filter: CommandFilter) -> Result<Vec<CommandRow>> {
        let user = filter.user.map(|user| user.0);
        let feeder = filter.feeder.map(|feeder| feeder.0);
        let state = filter.state.map(CommandState::code);
        let after = filter
            .after
            .map_or(Ok(0), |after| to_i64(after, "cursor"))?;
        let rows = sqlx::query_as!(
            Raw,
            r#"SELECT seq, key, user_id AS "user_id: Uuid", feeder_id AS "feeder_id: Uuid", kind,
                      body_hash, request_id, boot_count, state, outcome, feed, sent_ms, updated_ms
               FROM commands
               WHERE (?1 IS NULL OR user_id = ?1) AND (?2 IS NULL OR feeder_id = ?2)
                 AND (?3 IS NULL OR state = ?3) AND seq > ?4
               ORDER BY seq LIMIT ?5"#,
            user,
            feeder,
            state,
            after,
            filter.limit,
        )
        .fetch_all(&self.reader)
        .await?;

        rows.into_iter().map(CommandRow::try_from).collect()
    }
}

/// A row as SQLite holds it.
struct Raw {
    seq: i64,
    key: Vec<u8>,
    user_id: Uuid,
    feeder_id: Uuid,
    kind: String,
    body_hash: Vec<u8>,
    request_id: Vec<u8>,
    boot_count: Option<i64>,
    state: String,
    outcome: Option<String>,
    feed: Option<i64>,
    sent_ms: i64,
    updated_ms: i64,
}

impl TryFrom<Raw> for CommandRow {
    type Error = Error;

    fn try_from(raw: Raw) -> Result<Self> {
        let small = |value: i64, what| u32::try_from(value).map_err(|_| Error::Corrupt(what));
        let outcome = raw
            .outcome
            .map(|json| serde_json::from_str(&json).map_err(|_| Error::Corrupt("command outcome")))
            .transpose()?;

        Ok(Self {
            seq: from_i64(raw.seq, "command seq")?,
            key: raw
                .key
                .try_into()
                .map_err(|_| Error::Corrupt("command key"))?,
            user: UserId(raw.user_id),
            feeder: FeederId(raw.feeder_id),
            kind: CommandKind::from_code(&raw.kind).ok_or(Error::Corrupt("command kind"))?,
            body_hash: raw
                .body_hash
                .try_into()
                .map_err(|_| Error::Corrupt("command body hash"))?,
            request_id: raw
                .request_id
                .try_into()
                .map_err(|_| Error::Corrupt("command request id"))?,
            boot_count: raw
                .boot_count
                .map(|count| small(count, "boot count"))
                .transpose()?,
            state: CommandState::from_code(&raw.state).ok_or(Error::Corrupt("command state"))?,
            outcome,
            feed: raw
                .feed
                .map(|feed| small(feed, "feed number"))
                .transpose()?,
            sent: timestamp(raw.sent_ms)?,
            updated: timestamp(raw.updated_ms)?,
        })
    }
}

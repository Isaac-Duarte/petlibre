//! The audit log: the account changes the server records, and the command
//! journal, which joins the log through a trigger as each write is
//! recorded. A command's row is read with the journal's columns, so its
//! state and outcome are always the journal's.

use jiff::Timestamp;
use wellfed_api::{AuditAction, TokenId, UserId};
use uuid::Uuid;

use crate::commands::{CommandRow, Raw as CommandRaw};
use crate::convert::{from_i64, millis, timestamp, to_i64};
use crate::{Error, Result, Store};

/// An account change to record.
pub struct NewAudit<'a> {
    pub at: Timestamp,
    /// Who made it; `None` for the host's `reset-owner`.
    pub user: Option<UserId>,
    pub user_name: Option<&'a str>,
    pub token: Option<TokenId>,
    pub action: &'a AuditAction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditRow {
    /// Its place in the log: the list's order and cursor.
    pub seq: u64,
    pub at: Timestamp,
    pub user: Option<UserId>,
    pub user_name: Option<String>,
    pub token: Option<TokenId>,
    pub record: AuditRecord,
}

/// What a row holds: an account change, or a journal entry as it stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuditRecord {
    Change(AuditAction),
    Command(Box<CommandRow>),
}

/// Which rows [`Store::audit`] lists.
#[derive(Clone, Copy, Debug)]
pub struct AuditFilter {
    /// One user's rows only.
    pub user: Option<UserId>,
    /// Rows after this `seq`; from the first when `None`.
    pub after: Option<u64>,
    pub limit: u32,
}

impl Store {
    pub async fn record_audit(&self, entry: NewAudit<'_>) -> Result<()> {
        let at = millis(entry.at);
        let user = entry.user.map(|user| user.0);
        let token = entry.token.map(|token| token.0);
        let action =
            serde_json::to_string(entry.action).map_err(|error| Error::Json("audit", error))?;
        sqlx::query!(
            "INSERT INTO audit (at_ms, user_id, user_name, token_id, action)
             VALUES (?, ?, ?, ?, ?)",
            at,
            user,
            entry.user_name,
            token,
            action,
        )
        .execute(&self.writer)
        .await?;
        Ok(())
    }

    /// Up to `filter.limit` rows after `filter.after`, oldest first.
    pub async fn audit(&self, filter: AuditFilter) -> Result<Vec<AuditRow>> {
        let user = filter.user.map(|user| user.0);
        let after = filter
            .after
            .map_or(Ok(0), |after| to_i64(after, "cursor"))?;
        let rows = sqlx::query_as!(
            Joined,
            r#"SELECT audit.seq, audit.at_ms, audit.user_id AS "user_id: Uuid", audit.user_name,
                      audit.token_id AS "token_id: Uuid", audit.action,
                      commands.seq AS "c_seq?", commands.key AS "c_key?",
                      commands.user_id AS "c_user?: Uuid", commands.token_id AS "c_token?: Uuid",
                      commands.feeder_id AS "c_feeder?: Uuid", commands.kind AS "c_kind?",
                      commands.body_hash AS "c_body_hash?", commands.request_id AS "c_request_id?",
                      commands.boot_count AS "c_boot_count?",
                      commands.history_epoch AS "c_history_epoch?",
                      commands.history_seq AS "c_history_seq?", commands.state AS "c_state?",
                      commands.outcome AS "c_outcome?", commands.feed AS "c_feed?",
                      commands.sent_ms AS "c_sent_ms?", commands.updated_ms AS "c_updated_ms?"
               FROM audit LEFT JOIN commands ON commands.seq = audit.command_seq
               WHERE (?1 IS NULL OR audit.user_id = ?1) AND audit.seq > ?2
               ORDER BY audit.seq LIMIT ?3"#,
            user,
            after,
            filter.limit,
        )
        .fetch_all(&self.reader)
        .await?;

        rows.into_iter().map(AuditRow::try_from).collect()
    }
}

/// A row and its journal entry's columns, as SQLite holds them.
struct Joined {
    seq: i64,
    at_ms: i64,
    user_id: Option<Uuid>,
    user_name: Option<String>,
    token_id: Option<Uuid>,
    action: Option<String>,
    c_seq: Option<i64>,
    c_key: Option<Vec<u8>>,
    c_user: Option<Uuid>,
    c_token: Option<Uuid>,
    c_feeder: Option<Uuid>,
    c_kind: Option<String>,
    c_body_hash: Option<Vec<u8>>,
    c_request_id: Option<Vec<u8>>,
    c_boot_count: Option<i64>,
    c_history_epoch: Option<i64>,
    c_history_seq: Option<i64>,
    c_state: Option<String>,
    c_outcome: Option<String>,
    c_feed: Option<i64>,
    c_sent_ms: Option<i64>,
    c_updated_ms: Option<i64>,
}

impl Joined {
    /// The journal entry, when the row is one. Its required columns are
    /// all there or the row joined none.
    fn command(&mut self) -> Result<Option<CommandRow>> {
        let required = (
            self.c_seq,
            self.c_key.take(),
            self.c_user,
            self.c_feeder,
            self.c_kind.take(),
            self.c_body_hash.take(),
            self.c_request_id.take(),
            self.c_state.take(),
            self.c_sent_ms,
            self.c_updated_ms,
        );
        let (
            Some(seq),
            Some(key),
            Some(user_id),
            Some(feeder_id),
            Some(kind),
            Some(body_hash),
            Some(request_id),
            Some(state),
            Some(sent_ms),
            Some(updated_ms),
        ) = required
        else {
            return Ok(None);
        };

        let raw = CommandRaw {
            seq,
            key,
            user_id,
            token_id: self.c_token,
            feeder_id,
            kind,
            body_hash,
            request_id,
            boot_count: self.c_boot_count,
            history_epoch: self.c_history_epoch,
            history_seq: self.c_history_seq,
            state,
            outcome: self.c_outcome.take(),
            feed: self.c_feed,
            sent_ms,
            updated_ms,
        };
        CommandRow::try_from(raw).map(Some)
    }
}

impl TryFrom<Joined> for AuditRow {
    type Error = Error;

    fn try_from(mut joined: Joined) -> Result<Self> {
        let command = joined.command()?;
        let record = match (command, joined.action.as_deref()) {
            (Some(command), None) => AuditRecord::Command(Box::new(command)),
            (None, Some(json)) => AuditRecord::Change(
                serde_json::from_str(json).map_err(|_| Error::Corrupt("audit action"))?,
            ),
            _ => return Err(Error::Corrupt("audit row")),
        };

        Ok(Self {
            seq: from_i64(joined.seq, "audit seq")?,
            at: timestamp(joined.at_ms)?,
            user: joined.user_id.map(UserId),
            user_name: joined.user_name,
            token: joined.token_id.map(TokenId),
            record,
        })
    }
}

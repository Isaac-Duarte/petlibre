//! The phone keys the server made invites for, the revocations still owed
//! on them, and the clients it did not invite that an owner kept.

use jiff::Timestamp;
use wellfed_api::{
    ClientRole, FeederId, Hex, PhoneKey, RevokeAttempt, RevokeReason, Revoking, TokenId, UserId,
};
use uuid::Uuid;

use crate::convert::{millis, timestamp};
use crate::{Error, Result, Store};

mod reconcile;

/// A phone key about to be invited.
pub struct NewPhone {
    pub feeder: FeederId,
    pub key: [u8; 32],
    pub user: UserId,
    pub token: TokenId,
    pub role: ClientRole,
    pub invited: Timestamp,
    /// A bound on the invite's deadline while its answer is pending.
    pub invite_until: Timestamp,
}

/// What [`Store::record_phone`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recorded {
    /// The key is new: recorded.
    New,
    /// The key is already this token's, and not being revoked: asked again
    /// (a lost invite), its deadline raised to the new bound. `until` is
    /// the deadline it had.
    Again { until: Timestamp },
    /// The key is another token's, or being revoked: nothing changed.
    Taken,
}

/// Whose phone keys [`Store::revoke_phones`] marks.
#[derive(Clone, Copy, Debug)]
pub enum Whose {
    User(UserId),
    Token(TokenId),
    /// The token's other keys on the feeder: one key per token.
    Replaced {
        feeder: FeederId,
        token: TokenId,
        keep: [u8; 32],
    },
}

/// One `phone_keys` row as selected.
struct Raw {
    feeder_id: Uuid,
    key: Vec<u8>,
    user_id: Uuid,
    token_id: Option<Uuid>,
    role: String,
    invited_ms: i64,
    invite_until_ms: i64,
    revoke: Option<String>,
    revoke_ms: Option<i64>,
    attempt: Option<String>,
}

impl TryFrom<Raw> for PhoneKey {
    type Error = Error;

    fn try_from(raw: Raw) -> Result<Self> {
        let revoking = match (raw.revoke, raw.revoke_ms) {
            (Some(reason), Some(since)) => Some(Revoking {
                reason: RevokeReason::from_code(&reason).ok_or(Error::Corrupt("revoke reason"))?,
                since: timestamp(since)?,
                last: raw
                    .attempt
                    .map(|json| serde_json::from_str(&json))
                    .transpose()
                    .map_err(|_| Error::Corrupt("revoke attempt"))?,
            }),
            (None, None) => None,
            _ => return Err(Error::Corrupt("revocation without its time")),
        };
        Ok(Self {
            feeder: FeederId(raw.feeder_id),
            key: Hex(raw
                .key
                .try_into()
                .map_err(|_| Error::Corrupt("phone key"))?),
            user: UserId(raw.user_id),
            token: raw.token_id.map(TokenId),
            role: ClientRole::from_code(&raw.role).ok_or(Error::Corrupt("phone role"))?,
            invited: timestamp(raw.invited_ms)?,
            invite_until: timestamp(raw.invite_until_ms)?,
            revoking,
        })
    }
}

impl Store {
    /// Record a phone key for `phone.token` before its invite is asked
    /// for, unless another token holds it or it is being revoked.
    pub async fn record_phone(&self, phone: &NewPhone) -> Result<Recorded> {
        let key = phone.key.as_slice();
        let role = phone.role.code();
        let invited = millis(phone.invited);
        let until = millis(phone.invite_until);
        let mut tx = self.writer.begin().await?;
        let existing = sqlx::query!(
            r#"SELECT token_id AS "token_id: Uuid", invite_until_ms, revoke
               FROM phone_keys WHERE feeder_id = ? AND key = ?"#,
            phone.feeder.0,
            key,
        )
        .fetch_optional(&mut *tx)
        .await?;

        let recorded = match existing {
            None => {
                sqlx::query!(
                    "INSERT INTO phone_keys
                         (feeder_id, key, user_id, token_id, role, invited_ms, invite_until_ms)
                     VALUES (?, ?, ?, ?, ?, ?, ?)",
                    phone.feeder.0,
                    key,
                    phone.user.0,
                    phone.token.0,
                    role,
                    invited,
                    until,
                )
                .execute(&mut *tx)
                .await?;
                Recorded::New
            }
            Some(row) if row.token_id == Some(phone.token.0) && row.revoke.is_none() => {
                sqlx::query!(
                    "UPDATE phone_keys SET invite_until_ms = MAX(invite_until_ms, ?)
                     WHERE feeder_id = ? AND key = ?",
                    until,
                    phone.feeder.0,
                    key,
                )
                .execute(&mut *tx)
                .await?;
                Recorded::Again {
                    until: timestamp(row.invite_until_ms)?,
                }
            }
            Some(_) => Recorded::Taken,
        };
        tx.commit().await?;
        Ok(recorded)
    }

    /// Set the key's invite deadline, once the feeder has said it.
    pub async fn set_invite_until(
        &self,
        feeder: FeederId,
        key: &[u8; 32],
        until: Timestamp,
    ) -> Result<()> {
        let key = key.as_slice();
        let until = millis(until);
        sqlx::query!(
            "UPDATE phone_keys SET invite_until_ms = ? WHERE feeder_id = ? AND key = ?",
            until,
            feeder.0,
            key,
        )
        .execute(&self.writer)
        .await?;
        Ok(())
    }

    pub async fn phone(&self, feeder: FeederId, key: &[u8; 32]) -> Result<Option<PhoneKey>> {
        let key = key.as_slice();
        let row = sqlx::query_as!(
            Raw,
            r#"SELECT feeder_id AS "feeder_id: Uuid", key, user_id AS "user_id: Uuid",
                      token_id AS "token_id: Uuid", role, invited_ms, invite_until_ms,
                      revoke, revoke_ms, attempt
               FROM phone_keys WHERE feeder_id = ? AND key = ?"#,
            feeder.0,
            key,
        )
        .fetch_optional(&self.reader)
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    /// Every phone key recorded: one user's, or everyone's.
    pub async fn phones(&self, user: Option<UserId>) -> Result<Vec<PhoneKey>> {
        let user = user.map(|user| user.0);
        let rows = sqlx::query_as!(
            Raw,
            r#"SELECT feeder_id AS "feeder_id: Uuid", key, user_id AS "user_id: Uuid",
                      token_id AS "token_id: Uuid", role, invited_ms, invite_until_ms,
                      revoke, revoke_ms, attempt
               FROM phone_keys WHERE ?1 IS NULL OR user_id = ?1
               ORDER BY invited_ms, feeder_id, key"#,
            user,
        )
        .fetch_all(&self.reader)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    /// The phone keys recorded on one feeder.
    pub async fn feeder_phones(&self, feeder: FeederId) -> Result<Vec<PhoneKey>> {
        let rows = sqlx::query_as!(
            Raw,
            r#"SELECT feeder_id AS "feeder_id: Uuid", key, user_id AS "user_id: Uuid",
                      token_id AS "token_id: Uuid", role, invited_ms, invite_until_ms,
                      revoke, revoke_ms, attempt
               FROM phone_keys WHERE feeder_id = ? ORDER BY invited_ms, key"#,
            feeder.0,
        )
        .fetch_all(&self.reader)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    /// Mark `whose` phone keys for revocation, those not already marked;
    /// the feeders that hold any key so marked.
    pub async fn revoke_phones(
        &self,
        whose: Whose,
        reason: RevokeReason,
        at: Timestamp,
    ) -> Result<Vec<FeederId>> {
        let reason = reason.code();
        let at = millis(at);
        let (user, token, feeder, keep) = match whose {
            Whose::User(user) => (Some(user.0), None, None, None),
            Whose::Token(token) => (None, Some(token.0), None, None),
            Whose::Replaced {
                feeder,
                token,
                keep,
            } => (None, Some(token.0), Some(feeder.0), Some(keep.to_vec())),
        };
        let rows = sqlx::query!(
            r#"UPDATE phone_keys SET revoke = ?1, revoke_ms = ?2
               WHERE revoke IS NULL
                 AND (?3 IS NULL OR user_id = ?3)
                 AND (?4 IS NULL OR token_id = ?4)
                 AND (?5 IS NULL OR feeder_id = ?5)
                 AND (?6 IS NULL OR key != ?6)
                 AND (?3 IS NOT NULL OR ?4 IS NOT NULL)
               RETURNING feeder_id AS "feeder_id: Uuid""#,
            reason,
            at,
            user,
            token,
            feeder,
            keep,
        )
        .fetch_all(&self.writer)
        .await?;

        let mut feeders: Vec<FeederId> = rows
            .into_iter()
            .map(|row| FeederId(row.feeder_id))
            .collect();
        feeders.sort_unstable();
        feeders.dedup();
        Ok(feeders)
    }

    /// The phone keys on a feeder still to be revoked.
    pub async fn revoking(&self, feeder: FeederId) -> Result<Vec<PhoneKey>> {
        let phones = self.feeder_phones(feeder).await?;
        Ok(phones
            .into_iter()
            .filter(|phone| phone.revoking.is_some())
            .collect())
    }

    /// Record what an attempt to revoke a phone key came to.
    pub async fn revoke_attempt(
        &self,
        feeder: FeederId,
        key: &[u8; 32],
        attempt: &RevokeAttempt,
    ) -> Result<()> {
        let key = key.as_slice();
        let json = serde_json::to_string(attempt).map_err(|e| Error::Json("revoke attempt", e))?;
        sqlx::query!(
            "UPDATE phone_keys SET attempt = ? WHERE feeder_id = ? AND key = ?",
            json,
            feeder.0,
            key,
        )
        .execute(&self.writer)
        .await?;
        Ok(())
    }

    /// The feeder no longer holds the key: forget it as a phone's and as a
    /// kept client. Whether it was either.
    pub async fn forget_client(&self, feeder: FeederId, key: &[u8; 32]) -> Result<bool> {
        let key = key.as_slice();
        let mut tx = self.writer.begin().await?;
        let phone = sqlx::query!(
            "DELETE FROM phone_keys WHERE feeder_id = ? AND key = ?",
            feeder.0,
            key
        )
        .execute(&mut *tx)
        .await?;
        let kept = sqlx::query!(
            "DELETE FROM kept_clients WHERE feeder_id = ? AND key = ?",
            feeder.0,
            key
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(phone.rows_affected() + kept.rows_affected() > 0)
    }

    /// Keep a client the server did not invite.
    pub async fn keep_client(&self, feeder: FeederId, key: &[u8; 32], at: Timestamp) -> Result<()> {
        let key = key.as_slice();
        let at = millis(at);
        sqlx::query!(
            "INSERT INTO kept_clients (feeder_id, key, kept_ms) VALUES (?, ?, ?)
             ON CONFLICT DO NOTHING",
            feeder.0,
            key,
            at,
        )
        .execute(&self.writer)
        .await?;
        Ok(())
    }

    /// The clients an owner kept on a feeder.
    pub async fn kept_clients(&self, feeder: FeederId) -> Result<Vec<[u8; 32]>> {
        let rows = sqlx::query!(
            "SELECT key FROM kept_clients WHERE feeder_id = ? ORDER BY kept_ms",
            feeder.0
        )
        .fetch_all(&self.reader)
        .await?;
        rows.into_iter()
            .map(|row| {
                row.key
                    .try_into()
                    .map_err(|_| Error::Corrupt("kept client key"))
            })
            .collect()
    }
}

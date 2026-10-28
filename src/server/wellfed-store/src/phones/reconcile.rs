//! The reconcile: phone keys whose account or token no longer may hold
//! one, marked whatever change left them so.

use jiff::Timestamp;
use uuid::Uuid;
use wellfed_api::FeederId;

use crate::convert::millis;
use crate::{Result, Store};

impl Store {
    /// Mark every key whose account or token no longer may hold one,
    /// whatever change left it so: a deleted account or token, a viewer, an
    /// owner's key on an account now a member's, an account or token with
    /// an expiry. The account changes mark their keys themselves; this
    /// catches a key recorded while its account changed. The feeders that
    /// hold any key so marked.
    pub async fn reconcile_phones(&self, at: Timestamp) -> Result<Vec<FeederId>> {
        let at = millis(at);
        let mut tx = self.writer.begin().await?;
        let deleted = sqlx::query!(
            r#"UPDATE phone_keys SET revoke = 'deleted', revoke_ms = ?
               WHERE revoke IS NULL AND user_id NOT IN (SELECT id FROM users)
               RETURNING feeder_id AS "feeder_id: Uuid""#,
            at,
        )
        .fetch_all(&mut *tx)
        .await?;
        let token = sqlx::query!(
            r#"UPDATE phone_keys SET revoke = 'token_revoked', revoke_ms = ?
               WHERE revoke IS NULL AND token_id IS NOT NULL
                 AND token_id NOT IN (SELECT id FROM tokens)
               RETURNING feeder_id AS "feeder_id: Uuid""#,
            at,
        )
        .fetch_all(&mut *tx)
        .await?;
        let demoted = sqlx::query!(
            r#"UPDATE phone_keys SET revoke = 'demoted', revoke_ms = ?
               WHERE revoke IS NULL AND EXISTS (
                   SELECT 1 FROM users WHERE users.id = phone_keys.user_id
                     AND (users.role = 'viewer'
                          OR (users.role = 'member' AND phone_keys.role = 'owner')))
               RETURNING feeder_id AS "feeder_id: Uuid""#,
            at,
        )
        .fetch_all(&mut *tx)
        .await?;
        let expiring = sqlx::query!(
            r#"UPDATE phone_keys SET revoke = 'expiring', revoke_ms = ?
               WHERE revoke IS NULL AND (
                   EXISTS (SELECT 1 FROM users WHERE users.id = phone_keys.user_id
                             AND users.expires_ms IS NOT NULL)
                   OR EXISTS (SELECT 1 FROM tokens WHERE tokens.id = phone_keys.token_id
                                AND tokens.expires_ms IS NOT NULL))
               RETURNING feeder_id AS "feeder_id: Uuid""#,
            at,
        )
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;

        let mut feeders: Vec<FeederId> = deleted
            .into_iter()
            .map(|row| row.feeder_id)
            .chain(token.into_iter().map(|row| row.feeder_id))
            .chain(demoted.into_iter().map(|row| row.feeder_id))
            .chain(expiring.into_iter().map(|row| row.feeder_id))
            .map(FeederId)
            .collect();
        feeders.sort_unstable();
        feeders.dedup();
        Ok(feeders)
    }
}

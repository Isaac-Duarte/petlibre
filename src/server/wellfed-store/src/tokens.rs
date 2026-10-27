//! Bearer tokens: each one its user's, stored by the hash of its secret.

use jiff::Timestamp;
use wellfed_api::{Token, TokenId, User, UserId};
use uuid::Uuid;

use crate::convert::{millis, timestamp};
use crate::users::user;
use crate::{Result, Store};

pub struct NewToken<'a> {
    pub id: TokenId,
    /// SHA-256 of the secret's bytes.
    pub hash: &'a [u8; 32],
    pub user: UserId,
    pub name: &'a str,
    pub created: Timestamp,
    pub expires: Option<Timestamp>,
}

/// A presented token and the user it acts as, with their role and expiry
/// as they are now.
pub struct TokenUse {
    pub token: Token,
    pub user: User,
}

impl Store {
    pub async fn create_token(&self, token: NewToken<'_>) -> Result<()> {
        let hash = token.hash.as_slice();
        let (created, expires) = (millis(token.created), token.expires.map(millis));
        sqlx::query!(
            "INSERT INTO tokens (id, token_hash, user_id, name, created_ms, expires_ms)
             VALUES (?, ?, ?, ?, ?, ?)",
            token.id.0,
            hash,
            token.user.0,
            token.name,
            created,
            expires,
        )
        .execute(&self.writer)
        .await?;
        Ok(())
    }

    /// The token with this hash and its user, however it stands: the caller
    /// decides whether it, and the account, still hold.
    pub async fn token_use(&self, hash: &[u8; 32]) -> Result<Option<TokenUse>> {
        let hash = hash.as_slice();
        let row = sqlx::query!(
            r#"SELECT tokens.id AS "token_id: Uuid", tokens.name AS token_name,
                      tokens.created_ms, tokens.last_used_ms, tokens.expires_ms AS token_expires_ms,
                      users.id AS "user_id: Uuid", users.name AS user_name, users.role,
                      users.expires_ms AS user_expires_ms
               FROM tokens JOIN users ON users.id = tokens.user_id
               WHERE tokens.token_hash = ?"#,
            hash,
        )
        .fetch_optional(&self.reader)
        .await?;
        row.map(|row| {
            Ok(TokenUse {
                token: token(
                    row.token_id,
                    row.user_id,
                    row.token_name,
                    row.created_ms,
                    row.last_used_ms,
                    row.token_expires_ms,
                )?,
                user: user(row.user_id, row.user_name, &row.role, row.user_expires_ms)?,
            })
        })
        .transpose()
    }

    /// The tokens of `user`, or everyone's when `None`, oldest first.
    pub async fn tokens(&self, user: Option<UserId>) -> Result<Vec<Token>> {
        let user = user.map(|user| user.0);
        let rows = sqlx::query!(
            r#"SELECT id AS "id: Uuid", user_id AS "user_id: Uuid", name, created_ms,
                      last_used_ms, expires_ms
               FROM tokens WHERE (?1 IS NULL OR user_id = ?1)
               ORDER BY created_ms, rowid"#,
            user,
        )
        .fetch_all(&self.reader)
        .await?;
        rows.into_iter()
            .map(|row| {
                token(
                    row.id,
                    row.user_id,
                    row.name,
                    row.created_ms,
                    row.last_used_ms,
                    row.expires_ms,
                )
            })
            .collect()
    }

    pub async fn token(&self, id: TokenId) -> Result<Option<Token>> {
        let row = sqlx::query!(
            r#"SELECT id AS "id: Uuid", user_id AS "user_id: Uuid", name, created_ms,
                      last_used_ms, expires_ms
               FROM tokens WHERE id = ?"#,
            id.0,
        )
        .fetch_optional(&self.reader)
        .await?;
        row.map(|row| {
            token(
                row.id,
                row.user_id,
                row.name,
                row.created_ms,
                row.last_used_ms,
                row.expires_ms,
            )
        })
        .transpose()
    }

    /// Record that the token was used at `now`.
    pub async fn touch_token(&self, id: TokenId, now: Timestamp) -> Result<()> {
        let now = millis(now);
        sqlx::query!(
            "UPDATE tokens SET last_used_ms = ?1
             WHERE id = ?2 AND (last_used_ms IS NULL OR last_used_ms < ?1)",
            now,
            id.0,
        )
        .execute(&self.writer)
        .await?;
        Ok(())
    }

    /// Revoke a token; whether there was one.
    pub async fn delete_token(&self, id: TokenId) -> Result<bool> {
        let done = sqlx::query!("DELETE FROM tokens WHERE id = ?", id.0)
            .execute(&self.writer)
            .await?;
        Ok(done.rows_affected() == 1)
    }
}

fn token(
    id: Uuid,
    user: Uuid,
    name: String,
    created_ms: i64,
    last_used_ms: Option<i64>,
    expires_ms: Option<i64>,
) -> Result<Token> {
    Ok(Token {
        id: TokenId(id),
        user: UserId(user),
        name,
        created: timestamp(created_ms)?,
        last_used: last_used_ms.map(timestamp).transpose()?,
        expires: expires_ms.map(timestamp).transpose()?,
    })
}

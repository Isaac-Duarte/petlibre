//! Users and their browser sessions.

use jiff::Timestamp;
use wellfed_api::{Role, User, UserId};
use uuid::Uuid;

use crate::convert::millis;
use crate::{Error, Result, Store};

pub struct NewUser<'a> {
    pub id: UserId,
    pub name: &'a str,
    pub password_hash: &'a str,
    pub role: Role,
    pub created: Timestamp,
}

/// A user and the hash their password is checked against.
pub struct Login {
    pub user: User,
    pub password_hash: String,
}

impl Store {
    pub async fn users_exist(&self) -> Result<bool> {
        let row = sqlx::query!(r#"SELECT EXISTS (SELECT 1 FROM users) AS "exists!: bool""#)
            .fetch_one(&self.reader)
            .await?;
        Ok(row.exists)
    }

    /// Create `user` only while no user exists, in one statement, so two
    /// racing setups cannot both win. Whether it was created.
    pub async fn create_first_owner(&self, user: NewUser<'_>) -> Result<bool> {
        let created = millis(user.created);
        let role = user.role.code();
        let done = sqlx::query!(
            "INSERT INTO users (id, name, password_hash, role, created_ms)
             SELECT ?, ?, ?, ?, ? WHERE NOT EXISTS (SELECT 1 FROM users)",
            user.id.0,
            user.name,
            user.password_hash,
            role,
            created,
        )
        .execute(&self.writer)
        .await?;
        Ok(done.rows_affected() == 1)
    }

    pub async fn login(&self, name: &str) -> Result<Option<Login>> {
        let row = sqlx::query!(
            r#"SELECT id AS "id: Uuid", name, password_hash, role FROM users WHERE name = ?"#,
            name
        )
        .fetch_optional(&self.reader)
        .await?;
        row.map(|row| {
            Ok(Login {
                user: User {
                    id: UserId(row.id),
                    name: row.name,
                    role: Role::from_code(&row.role).ok_or(Error::Corrupt("role"))?,
                },
                password_hash: row.password_hash,
            })
        })
        .transpose()
    }

    pub async fn create_session(
        &self,
        token_hash: &[u8; 32],
        user: UserId,
        created: Timestamp,
        expires: Timestamp,
    ) -> Result<()> {
        let hash = token_hash.as_slice();
        let (created, expires) = (millis(created), millis(expires));
        sqlx::query!(
            "INSERT INTO sessions (token_hash, user_id, created_ms, expires_ms) VALUES (?, ?, ?, ?)",
            hash,
            user.0,
            created,
            expires,
        )
        .execute(&self.writer)
        .await?;
        Ok(())
    }

    /// The user an unexpired session belongs to.
    pub async fn session_user(
        &self,
        token_hash: &[u8; 32],
        now: Timestamp,
    ) -> Result<Option<User>> {
        let hash = token_hash.as_slice();
        let now = millis(now);
        let row = sqlx::query!(
            r#"SELECT users.id AS "id: Uuid", users.name, users.role
               FROM sessions JOIN users ON users.id = sessions.user_id
               WHERE sessions.token_hash = ? AND sessions.expires_ms > ?"#,
            hash,
            now,
        )
        .fetch_optional(&self.reader)
        .await?;
        row.map(|row| {
            Ok(User {
                id: UserId(row.id),
                name: row.name,
                role: Role::from_code(&row.role).ok_or(Error::Corrupt("role"))?,
            })
        })
        .transpose()
    }

    pub async fn delete_session(&self, token_hash: &[u8; 32]) -> Result<()> {
        let hash = token_hash.as_slice();
        sqlx::query!("DELETE FROM sessions WHERE token_hash = ?", hash)
            .execute(&self.writer)
            .await?;
        Ok(())
    }

    pub async fn delete_expired_sessions(&self, now: Timestamp) -> Result<u64> {
        let now = millis(now);
        let done = sqlx::query!("DELETE FROM sessions WHERE expires_ms <= ?", now)
            .execute(&self.writer)
            .await?;
        Ok(done.rows_affected())
    }
}

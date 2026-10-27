//! Users, the household they share, and their browser sessions.

use jiff::Timestamp;
use wellfed_api::{Household, Role, TimeFormat, Units, User, UserId};
use uuid::Uuid;

use crate::convert::{millis, timestamp};
use crate::{Error, Result, Store};

pub struct NewUser<'a> {
    pub id: UserId,
    pub name: &'a str,
    pub password_hash: &'a str,
    pub role: Role,
    pub expires: Option<Timestamp>,
    pub created: Timestamp,
}

/// A user and the hash their password is checked against.
pub struct Login {
    pub user: User,
    pub password_hash: String,
}

/// A session and the user it belongs to.
pub struct SessionRow {
    pub user: User,
    pub last_used: Timestamp,
}

/// What became of a change to an account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Done,
    NotFound,
    /// Refused: no owner would be left.
    LastOwner,
}

impl Store {
    pub async fn users_exist(&self) -> Result<bool> {
        let row = sqlx::query!(r#"SELECT EXISTS (SELECT 1 FROM users) AS "exists!: bool""#)
            .fetch_one(&self.reader)
            .await?;
        Ok(row.exists)
    }

    /// Create `user` and the household only while no user exists, in one
    /// transaction, so two racing setups cannot both win. Whether it was
    /// created.
    pub async fn create_first_owner(
        &self,
        user: NewUser<'_>,
        household: &Household,
    ) -> Result<bool> {
        let (created, expires) = (millis(user.created), user.expires.map(millis));
        let role = user.role.code();
        let (units, format) = (household.units.code(), household.time_format.code());

        let mut tx = self.writer.begin().await?;
        let done = sqlx::query!(
            "INSERT INTO users (id, name, password_hash, role, expires_ms, created_ms)
             SELECT ?, ?, ?, ?, ?, ? WHERE NOT EXISTS (SELECT 1 FROM users)",
            user.id.0,
            user.name,
            user.password_hash,
            role,
            expires,
            created,
        )
        .execute(&mut *tx)
        .await?;
        if done.rows_affected() != 1 {
            return Ok(false);
        }
        sqlx::query!(
            "INSERT OR REPLACE INTO household (id, name, units, time_format, zone)
             VALUES (1, ?, ?, ?, ?)",
            household.name,
            units,
            format,
            household.zone,
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn login(&self, name: &str) -> Result<Option<Login>> {
        let row = sqlx::query!(
            r#"SELECT id AS "id: Uuid", name, password_hash, role, expires_ms
               FROM users WHERE name = ?"#,
            name
        )
        .fetch_optional(&self.reader)
        .await?;
        row.map(|row| {
            Ok(Login {
                user: user(row.id, row.name, &row.role, row.expires_ms)?,
                password_hash: row.password_hash,
            })
        })
        .transpose()
    }

    /// Every user, by name.
    pub async fn users(&self) -> Result<Vec<User>> {
        let rows = sqlx::query!(
            r#"SELECT id AS "id: Uuid", name, role, expires_ms FROM users ORDER BY name"#
        )
        .fetch_all(&self.reader)
        .await?;
        rows.into_iter()
            .map(|row| user(row.id, row.name, &row.role, row.expires_ms))
            .collect()
    }

    pub async fn user(&self, id: UserId) -> Result<Option<User>> {
        let row = sqlx::query!(
            r#"SELECT id AS "id: Uuid", name, role, expires_ms FROM users WHERE id = ?"#,
            id.0
        )
        .fetch_optional(&self.reader)
        .await?;
        row.map(|row| user(row.id, row.name, &row.role, row.expires_ms))
            .transpose()
    }

    /// Add a user; [`Error::Duplicate`] when the name is taken.
    pub async fn create_user(&self, user: NewUser<'_>) -> Result<()> {
        let (created, expires) = (millis(user.created), user.expires.map(millis));
        let role = user.role.code();
        let inserted = sqlx::query!(
            "INSERT INTO users (id, name, password_hash, role, expires_ms, created_ms)
             VALUES (?, ?, ?, ?, ?, ?)",
            user.id.0,
            user.name,
            user.password_hash,
            role,
            expires,
            created,
        )
        .execute(&self.writer)
        .await;
        match inserted {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(error)) if error.is_unique_violation() => {
                Err(Error::Duplicate("user"))
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Set a user's role and expiry, unless that demotes the last owner.
    pub async fn update_user(
        &self,
        id: UserId,
        role: Role,
        expires: Option<Timestamp>,
    ) -> Result<Change> {
        let (code, expires) = (role.code(), expires.map(millis));

        let mut tx = self.writer.begin().await?;
        if let Some(refused) = guard_owner(&mut tx, id, Some(role)).await? {
            return Ok(refused);
        }
        sqlx::query!(
            "UPDATE users SET role = ?, expires_ms = ? WHERE id = ?",
            code,
            expires,
            id.0,
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Change::Done)
    }

    /// Delete a user, and with them their sessions and tokens, unless
    /// they are the last owner.
    pub async fn delete_user(&self, id: UserId) -> Result<Change> {
        let mut tx = self.writer.begin().await?;
        if let Some(refused) = guard_owner(&mut tx, id, None).await? {
            return Ok(refused);
        }
        sqlx::query!("DELETE FROM users WHERE id = ?", id.0)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Change::Done)
    }

    /// Replace a user's password hash; whether the user exists.
    pub async fn set_password(&self, id: UserId, password_hash: &str) -> Result<bool> {
        let done = sqlx::query!(
            "UPDATE users SET password_hash = ? WHERE id = ?",
            password_hash,
            id.0
        )
        .execute(&self.writer)
        .await?;
        Ok(done.rows_affected() == 1)
    }

    pub async fn household(&self) -> Result<Option<Household>> {
        let row = sqlx::query!("SELECT name, units, time_format, zone FROM household")
            .fetch_optional(&self.reader)
            .await?;
        row.map(|row| {
            Ok(Household {
                name: row.name,
                units: Units::from_code(&row.units).ok_or(Error::Corrupt("units"))?,
                time_format: TimeFormat::from_code(&row.time_format)
                    .ok_or(Error::Corrupt("time format"))?,
                zone: row.zone,
            })
        })
        .transpose()
    }

    pub async fn set_household(&self, household: &Household) -> Result<()> {
        let (units, format) = (household.units.code(), household.time_format.code());
        sqlx::query!(
            "INSERT OR REPLACE INTO household (id, name, units, time_format, zone)
             VALUES (1, ?, ?, ?, ?)",
            household.name,
            units,
            format,
            household.zone,
        )
        .execute(&self.writer)
        .await?;
        Ok(())
    }

    pub async fn create_session(
        &self,
        token_hash: &[u8; 32],
        user: UserId,
        now: Timestamp,
    ) -> Result<()> {
        let hash = token_hash.as_slice();
        let now = millis(now);
        sqlx::query!(
            "INSERT INTO sessions (token_hash, user_id, created_ms, last_used_ms)
             VALUES (?, ?, ?, ?)",
            hash,
            user.0,
            now,
            now,
        )
        .execute(&self.writer)
        .await?;
        Ok(())
    }

    /// The session with this token, and its user, however long ago it was
    /// last used: the caller decides whether it still holds.
    pub async fn session(&self, token_hash: &[u8; 32]) -> Result<Option<SessionRow>> {
        let hash = token_hash.as_slice();
        let row = sqlx::query!(
            r#"SELECT users.id AS "id: Uuid", users.name, users.role, users.expires_ms,
                      sessions.last_used_ms
               FROM sessions JOIN users ON users.id = sessions.user_id
               WHERE sessions.token_hash = ?"#,
            hash,
        )
        .fetch_optional(&self.reader)
        .await?;
        row.map(|row| {
            Ok(SessionRow {
                user: user(row.id, row.name, &row.role, row.expires_ms)?,
                last_used: timestamp(row.last_used_ms)?,
            })
        })
        .transpose()
    }

    /// Record that the session was used at `now`.
    pub async fn touch_session(&self, token_hash: &[u8; 32], now: Timestamp) -> Result<()> {
        let hash = token_hash.as_slice();
        let now = millis(now);
        sqlx::query!(
            "UPDATE sessions SET last_used_ms = ? WHERE token_hash = ? AND last_used_ms < ?",
            now,
            hash,
            now,
        )
        .execute(&self.writer)
        .await?;
        Ok(())
    }

    pub async fn delete_session(&self, token_hash: &[u8; 32]) -> Result<()> {
        let hash = token_hash.as_slice();
        sqlx::query!("DELETE FROM sessions WHERE token_hash = ?", hash)
            .execute(&self.writer)
            .await?;
        Ok(())
    }

    /// End every session of `user` but `except`; how many there were.
    pub async fn delete_sessions_of(&self, user: UserId, except: Option<&[u8; 32]>) -> Result<u64> {
        let keep = except.map(<[u8; 32]>::as_slice);
        let done = sqlx::query!(
            "DELETE FROM sessions WHERE user_id = ? AND token_hash IS NOT ?",
            user.0,
            keep,
        )
        .execute(&self.writer)
        .await?;
        Ok(done.rows_affected())
    }

    /// The hash a user's password is checked against.
    pub async fn password_hash(&self, id: UserId) -> Result<Option<String>> {
        let row = sqlx::query!("SELECT password_hash FROM users WHERE id = ?", id.0)
            .fetch_optional(&self.reader)
            .await?;
        Ok(row.map(|row| row.password_hash))
    }

    /// End every session last used at or before `cutoff`.
    pub async fn delete_idle_sessions(&self, cutoff: Timestamp) -> Result<u64> {
        let cutoff = millis(cutoff);
        let done = sqlx::query!("DELETE FROM sessions WHERE last_used_ms <= ?", cutoff)
            .execute(&self.writer)
            .await?;
        Ok(done.rows_affected())
    }
}

/// Whether the change of user `id` to `role` (`None`: deleting them) must
/// be refused, inside the change's transaction: `NotFound` for no such
/// user, `LastOwner` when they are the only owner and would stop being one.
async fn guard_owner(
    tx: &mut sqlx::SqliteConnection,
    id: UserId,
    role: Option<Role>,
) -> Result<Option<Change>> {
    let current = sqlx::query!("SELECT role FROM users WHERE id = ?", id.0)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(current) = current else {
        return Ok(Some(Change::NotFound));
    };
    if current.role != Role::Owner.code() || role == Some(Role::Owner) {
        return Ok(None);
    }

    let owner = Role::Owner.code();
    let owners = sqlx::query!(
        r#"SELECT COUNT(*) AS "count!: i64" FROM users WHERE role = ?"#,
        owner
    )
    .fetch_one(&mut *tx)
    .await?;
    Ok((owners.count <= 1).then_some(Change::LastOwner))
}

pub(crate) fn user(id: Uuid, name: String, role: &str, expires_ms: Option<i64>) -> Result<User> {
    Ok(User {
        id: UserId(id),
        name,
        role: Role::from_code(role).ok_or(Error::Corrupt("role"))?,
        expires: expires_ms.map(timestamp).transpose()?,
    })
}

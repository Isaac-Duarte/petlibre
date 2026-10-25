//! The owner's server database, and the only crate with SQL in it
//! ([API design](../../../../docs/api/README.md#storage)).
//!
//! SQLite in WAL mode, with one writer connection and a small read pool,
//! so writes never contend. Migrations are embedded and run at open, after
//! a copy of the database is taken if any are pending. Queries are
//! `query!`/`query_as!`, checked at build time against the offline
//! metadata in `.sqlx/` (`make sqlx-prepare` regenerates it).
//!
//! Times cross this boundary as [`jiff::Timestamp`] and are stored as Unix
//! milliseconds. Event bodies and sample values are stored as the feeder
//! sent them (postcard bytes), so the mirror is raw; the service decodes
//! them.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::Timestamp;
use wellfed_api::{
    ClientRole, EventKind, FeederId, FeederStatus, Gap, MetricKind, Role, Settings, User, UserId,
};
use sqlx::migrate::{Migrate, Migrator};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
use uuid::Uuid;

static MIGRATOR: Migrator = sqlx::migrate!();

/// How long a connection waits on a lock before failing.
const BUSY: Duration = Duration::from_secs(5);
const READERS: u32 = 4;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("migrations: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("a stored {0} is out of range")]
    Corrupt(&'static str),
    #[error("{0} does not fit the database")]
    TooLarge(&'static str),
    #[error("that {0} already exists")]
    Duplicate(&'static str),
    #[error("encode a cached {0}: {1}")]
    Json(&'static str, serde_json::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The database: a writer and a read pool over one file.
#[derive(Clone, Debug)]
pub struct Store {
    writer: SqlitePool,
    reader: SqlitePool,
}

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

/// The server's feeder key, sealed by the service.
#[derive(Clone, PartialEq, Eq)]
pub struct SealedKey {
    pub nonce: [u8; 12],
    pub sealed: Vec<u8>,
}

impl std::fmt::Debug for SealedKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SealedKey").finish_non_exhaustive()
    }
}

pub struct NewFeeder<'a> {
    pub id: FeederId,
    pub identity: [u8; 32],
    pub name: &'a str,
    pub role: ClientRole,
    pub address: Option<&'a str>,
    pub key: SealedKey,
    pub created: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeederRow {
    pub id: FeederId,
    pub identity: [u8; 32],
    pub name: String,
    pub role: ClientRole,
    pub address: Option<String>,
    pub key: SealedKey,
    /// The newest event held.
    pub synced: Option<u64>,
}

/// The last status and settings read, and when.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeederState {
    pub status: Option<(FeederStatus, Timestamp)>,
    pub settings: Option<(Settings, Timestamp)>,
}

pub struct NewEvent<'a> {
    pub seq: u64,
    pub at: Option<Timestamp>,
    pub received: Timestamp,
    pub kind: EventKind,
    /// The body's postcard bytes.
    pub body: &'a [u8],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventRow {
    pub seq: u64,
    pub at: Option<Timestamp>,
    pub received: Timestamp,
    pub kind: EventKind,
    pub body: Vec<u8>,
}

pub struct NewSample<'a> {
    pub received: Timestamp,
    pub uptime_ms: u32,
    pub metric: MetricKind,
    /// The metric's postcard bytes.
    pub value: &'a [u8],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampleRow {
    pub received: Timestamp,
    pub uptime_ms: u32,
    pub metric: MetricKind,
    pub value: Vec<u8>,
}

impl Store {
    /// Open (or create) the database at `path` and bring its schema up to
    /// date. Before pending migrations run on an existing database, it is
    /// copied beside itself as `<name>.before-<version>-<unix ms>`: a copy per
    /// attempt, so one left by a migration that failed never blocks the next.
    pub async fn open(path: &Path) -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(BUSY);
        let writer = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options.clone())
            .await?;
        migrate(&writer, path).await?;

        let reader = SqlitePoolOptions::new()
            .max_connections(READERS)
            .connect_with(options.read_only(true))
            .await?;
        Ok(Self { writer, reader })
    }

    /// Wait for both pools to close, so the WAL is checkpointed.
    pub async fn close(&self) {
        self.reader.close().await;
        self.writer.close().await;
    }

    // Users and sessions.

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

    // Feeders.

    /// Record an adopted feeder; [`Error::Duplicate`] if its identity is
    /// already adopted.
    pub async fn insert_feeder(&self, feeder: NewFeeder<'_>) -> Result<()> {
        let identity = feeder.identity.as_slice();
        let role = feeder.role.code();
        let nonce = feeder.key.nonce.as_slice();
        let created = millis(feeder.created);
        let done = sqlx::query!(
            "INSERT INTO feeders (id, identity, name, role, address, key_nonce, key_sealed, created_ms)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            feeder.id.0,
            identity,
            feeder.name,
            role,
            feeder.address,
            nonce,
            feeder.key.sealed,
            created,
        )
        .execute(&self.writer)
        .await;
        match done {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(error)) if error.is_unique_violation() => {
                Err(Error::Duplicate("feeder"))
            }
            Err(error) => Err(error.into()),
        }
    }

    pub async fn feeders(&self) -> Result<Vec<FeederRow>> {
        let rows = sqlx::query!(
            r#"SELECT id AS "id: Uuid", identity, name, role, address, key_nonce, key_sealed, synced
               FROM feeders ORDER BY created_ms, id"#
        )
        .fetch_all(&self.reader)
        .await?;
        rows.into_iter()
            .map(|row| {
                feeder_row(
                    row.id,
                    &row.identity,
                    row.name,
                    &row.role,
                    row.address,
                    &row.key_nonce,
                    row.key_sealed,
                    row.synced,
                )
            })
            .collect()
    }

    pub async fn feeder(&self, id: FeederId) -> Result<Option<FeederRow>> {
        let row = sqlx::query!(
            r#"SELECT id AS "id: Uuid", identity, name, role, address, key_nonce, key_sealed, synced
               FROM feeders WHERE id = ?"#,
            id.0
        )
        .fetch_optional(&self.reader)
        .await?;
        row.map(|row| {
            feeder_row(
                row.id,
                &row.identity,
                row.name,
                &row.role,
                row.address,
                &row.key_nonce,
                row.key_sealed,
                row.synced,
            )
        })
        .transpose()
    }

    pub async fn feeder_adopted(&self, identity: &[u8; 32]) -> Result<bool> {
        let identity = identity.as_slice();
        let row = sqlx::query!(
            r#"SELECT EXISTS (SELECT 1 FROM feeders WHERE identity = ?) AS "exists!: bool""#,
            identity
        )
        .fetch_one(&self.reader)
        .await?;
        Ok(row.exists)
    }

    /// Where the feeder was last reached.
    pub async fn set_address(&self, id: FeederId, address: &str) -> Result<()> {
        sqlx::query!("UPDATE feeders SET address = ? WHERE id = ?", address, id.0)
            .execute(&self.writer)
            .await?;
        Ok(())
    }

    pub async fn record_status(
        &self,
        id: FeederId,
        status: &FeederStatus,
        at: Timestamp,
    ) -> Result<()> {
        let json = serde_json::to_string(status).map_err(|e| Error::Json("status", e))?;
        let at = millis(at);
        sqlx::query!(
            "INSERT INTO feeder_state (feeder_id, status, status_ms) VALUES (?, ?, ?)
             ON CONFLICT (feeder_id) DO UPDATE SET status = excluded.status, status_ms = excluded.status_ms",
            id.0,
            json,
            at,
        )
        .execute(&self.writer)
        .await?;
        Ok(())
    }

    pub async fn record_settings(
        &self,
        id: FeederId,
        settings: &Settings,
        at: Timestamp,
    ) -> Result<()> {
        let json = serde_json::to_string(settings).map_err(|e| Error::Json("settings", e))?;
        let at = millis(at);
        sqlx::query!(
            "INSERT INTO feeder_state (feeder_id, settings, settings_ms) VALUES (?, ?, ?)
             ON CONFLICT (feeder_id) DO UPDATE SET settings = excluded.settings, settings_ms = excluded.settings_ms",
            id.0,
            json,
            at,
        )
        .execute(&self.writer)
        .await?;
        Ok(())
    }

    pub async fn state(&self, id: FeederId) -> Result<FeederState> {
        let row = sqlx::query!(
            "SELECT status, status_ms, settings, settings_ms FROM feeder_state WHERE feeder_id = ?",
            id.0
        )
        .fetch_optional(&self.reader)
        .await?;
        let Some(row) = row else {
            return Ok(FeederState::default());
        };
        Ok(FeederState {
            status: cached(row.status, row.status_ms, "status")?,
            settings: cached(row.settings, row.settings_ms, "settings")?,
        })
    }

    // History.

    /// Record one event and move the feeder's cursor past it, together.
    /// An event already held changes nothing; whether it was new.
    pub async fn record_event(&self, id: FeederId, event: NewEvent<'_>) -> Result<bool> {
        let seq = to_i64(event.seq, "event sequence")?;
        let at = event.at.map(|at| at.as_second());
        let received = millis(event.received);
        let kind = event.kind.code();

        let mut tx = self.writer.begin().await?;
        let done = sqlx::query!(
            "INSERT INTO events (feeder_id, seq, at_s, received_ms, kind, body)
             VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
            id.0,
            seq,
            at,
            received,
            kind,
            event.body,
        )
        .execute(&mut *tx)
        .await?;
        advance(&mut tx, id, seq).await?;
        tx.commit().await?;
        Ok(done.rows_affected() == 1)
    }

    /// Record that `from..=to` are lost, and move the cursor past them.
    pub async fn record_gap(
        &self,
        id: FeederId,
        from: u64,
        to: u64,
        received: Timestamp,
    ) -> Result<()> {
        let from = to_i64(from, "gap start")?;
        let to = to_i64(to, "gap end")?;
        let received = millis(received);

        let mut tx = self.writer.begin().await?;
        sqlx::query!(
            "INSERT INTO gaps (feeder_id, from_seq, to_seq, received_ms) VALUES (?, ?, ?, ?)
             ON CONFLICT DO NOTHING",
            id.0,
            from,
            to,
            received,
        )
        .execute(&mut *tx)
        .await?;
        advance(&mut tx, id, to).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Up to `limit` events after `after` (from the oldest when `None`),
    /// oldest first, of `kind` only when given.
    pub async fn events(
        &self,
        id: FeederId,
        after: Option<u64>,
        limit: u32,
        kind: Option<EventKind>,
    ) -> Result<Vec<EventRow>> {
        let after = after.map_or(Ok(-1), |after| to_i64(after, "cursor"))?;
        let kind = kind.map(EventKind::code);
        let rows = sqlx::query!(
            r#"SELECT seq, at_s, received_ms, kind, body FROM events
               WHERE feeder_id = ?1 AND seq > ?2 AND (?3 IS NULL OR kind = ?3)
               ORDER BY seq LIMIT ?4"#,
            id.0,
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
                    at: row
                        .at_s
                        .map(|s| {
                            Timestamp::from_second(s).map_err(|_| Error::Corrupt("event time"))
                        })
                        .transpose()?,
                    received: timestamp(row.received_ms)?,
                    kind: EventKind::from_code(&row.kind).ok_or(Error::Corrupt("event kind"))?,
                    body: row.body,
                })
            })
            .collect()
    }

    /// The gaps that end after `after` and at or before `through`.
    pub async fn gaps(&self, id: FeederId, after: Option<u64>, through: u64) -> Result<Vec<Gap>> {
        let after = after.map_or(Ok(-1), |after| to_i64(after, "cursor"))?;
        let through = to_i64(through, "cursor")?;
        let rows = sqlx::query!(
            "SELECT from_seq, to_seq, received_ms FROM gaps
             WHERE feeder_id = ? AND to_seq > ? AND to_seq <= ? ORDER BY from_seq",
            id.0,
            after,
            through,
        )
        .fetch_all(&self.reader)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(Gap {
                    from: from_i64(row.from_seq, "gap start")?,
                    to: from_i64(row.to_seq, "gap end")?,
                    received: timestamp(row.received_ms)?,
                })
            })
            .collect()
    }

    pub async fn record_sample(&self, id: FeederId, sample: NewSample<'_>) -> Result<()> {
        let received = millis(sample.received);
        let metric = sample.metric.code();
        sqlx::query!(
            "INSERT INTO samples (feeder_id, received_ms, uptime_ms, metric, value) VALUES (?, ?, ?, ?, ?)",
            id.0,
            received,
            sample.uptime_ms,
            metric,
            sample.value,
        )
        .execute(&self.writer)
        .await?;
        Ok(())
    }

    /// Up to `limit` samples of `metric`, newest first.
    pub async fn samples(
        &self,
        id: FeederId,
        metric: MetricKind,
        limit: u32,
    ) -> Result<Vec<SampleRow>> {
        let code = metric.code();
        let rows = sqlx::query!(
            "SELECT received_ms, uptime_ms, value FROM samples
             WHERE feeder_id = ? AND metric = ? ORDER BY received_ms DESC, rowid DESC LIMIT ?",
            id.0,
            code,
            limit,
        )
        .fetch_all(&self.reader)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(SampleRow {
                    received: timestamp(row.received_ms)?,
                    uptime_ms: u32::try_from(row.uptime_ms)
                        .map_err(|_| Error::Corrupt("uptime"))?,
                    metric,
                    value: row.value,
                })
            })
            .collect()
    }
}

/// Apply pending migrations, copying an existing database first.
async fn migrate(writer: &SqlitePool, path: &Path) -> Result<()> {
    let mut connection = writer.acquire().await?;
    connection
        .ensure_migrations_table(&MIGRATOR.table_name)
        .await?;
    let applied: Vec<i64> = connection
        .list_applied_migrations(&MIGRATOR.table_name)
        .await?
        .into_iter()
        .map(|migration| migration.version)
        .collect();
    let pending = MIGRATOR
        .iter()
        .filter(|migration| !applied.contains(&migration.version))
        .map(|migration| migration.version)
        .max();
    if let (Some(version), false) = (pending, applied.is_empty()) {
        let copy = backup_path(path, version);
        // `VACUUM INTO` takes a consistent copy of a live database. It is
        // not a table query, so it has no offline metadata to check.
        sqlx::query("VACUUM INTO ?")
            .bind(copy.to_string_lossy().into_owned())
            .execute(&mut *connection)
            .await?;
    }
    drop(connection);

    MIGRATOR.run(writer).await?;
    Ok(())
}

fn backup_path(path: &Path, version: i64) -> PathBuf {
    let now = Timestamp::now().as_millisecond();
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".before-{version}-{now}"));
    path.with_file_name(name)
}

/// Move the cursor up to `seq`, never back.
async fn advance(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    id: FeederId,
    seq: i64,
) -> Result<()> {
    sqlx::query!(
        "UPDATE feeders SET synced = MAX(COALESCE(synced, -1), ?) WHERE id = ?",
        seq,
        id.0
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "one argument per selected column"
)]
fn feeder_row(
    id: Uuid,
    identity: &[u8],
    name: String,
    role: &str,
    address: Option<String>,
    nonce: &[u8],
    sealed: Vec<u8>,
    synced: Option<i64>,
) -> Result<FeederRow> {
    Ok(FeederRow {
        id: FeederId(id),
        identity: identity
            .try_into()
            .map_err(|_| Error::Corrupt("identity"))?,
        name,
        role: ClientRole::from_code(role).ok_or(Error::Corrupt("feeder role"))?,
        address,
        key: SealedKey {
            nonce: nonce.try_into().map_err(|_| Error::Corrupt("key nonce"))?,
            sealed,
        },
        synced: synced.map(|seq| from_i64(seq, "cursor")).transpose()?,
    })
}

fn cached<T: serde::de::DeserializeOwned>(
    json: Option<String>,
    at: Option<i64>,
    what: &'static str,
) -> Result<Option<(T, Timestamp)>> {
    let (Some(json), Some(at)) = (json, at) else {
        return Ok(None);
    };
    let value = serde_json::from_str(&json).map_err(|_| Error::Corrupt(what))?;
    Ok(Some((value, timestamp(at)?)))
}

fn millis(at: Timestamp) -> i64 {
    at.as_millisecond()
}

fn timestamp(ms: i64) -> Result<Timestamp> {
    Timestamp::from_millisecond(ms).map_err(|_| Error::Corrupt("time"))
}

fn to_i64(value: u64, what: &'static str) -> Result<i64> {
    i64::try_from(value).map_err(|_| Error::TooLarge(what))
}

fn from_i64(value: i64, what: &'static str) -> Result<u64> {
    u64::try_from(value).map_err(|_| Error::Corrupt(what))
}

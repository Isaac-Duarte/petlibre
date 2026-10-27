//! The owner's server database, and the only crate with SQL in it
//! ([API design](../../../../docs/api/README.md#storage)).
//!
//! SQLite in WAL mode, with one writer connection and a small read pool,
//! so writes never contend. Migrations are embedded and run at open, after
//! a copy of the database is taken if any are pending. Queries are
//! `query!`/`query_as!`, checked at build time against the offline
//! metadata in `.sqlx/` (`make sqlx-prepare` regenerates it).
//!
//! Each table group has its own module of `impl Store` methods and row
//! types; the conversions between Rust and SQLite values live in
//! `convert`. Times cross this boundary as [`jiff::Timestamp`] and are
//! stored as Unix milliseconds. Event bodies and sample values are stored
//! as the feeder sent them (postcard bytes), so the mirror is raw. Beside
//! the raw events, each event's typed projection (feeds, lock changes,
//! boots, clock syncs, feeding blocked) is written in the same transaction
//! for the views to query, and rebuilt from the raw rows when the
//! projection code changes. Each sample carries its number beside its
//! bytes, and hourly rollups of the numbers outlive the raw rows the
//! owner's retention removes. Feed traces and statistics are read from the
//! feeder apart from its history, so they are kept beside it, never
//! rebuilt.

#![forbid(unsafe_code)]

mod audit;
mod commands;
mod convert;
mod diagnostics;
mod feeders;
mod feeds;
mod foods;
mod history;
mod installs;
mod jobs;
mod migrate;
mod pets;
mod projections;
mod refills;
mod samples;
mod tokens;
mod users;

use std::path::Path;
use std::time::Duration;

use jiff::Timestamp;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};

pub use audit::{AuditFilter, AuditRecord, AuditRow, NewAudit};
pub use commands::{CommandFilter, CommandRow, Journaled, NewCommand};
pub use diagnostics::{FoodReading, RawTrace, StatsRow, TraceFound, TraceRow};
pub use feeders::{FeederRow, FeederState, NewFeeder, SealedKey, Synced};
pub use foods::FoodSet;
pub use history::{EventRow, NewEvent};
pub use pets::WeightCursor;
pub use projections::{Boot, ClockSync, Feed, LockChange, PROJECTION_VERSION, Projected, Refresh};
pub use samples::{HourRow, NewSample, SampleKey, SampleRefresh, SampleRow, Span};
pub use tokens::{NewToken, TokenUse};
pub use users::{Change, Login, NewUser, SessionRow};

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
    #[error("a {0} still in use cannot be deleted")]
    InUse(&'static str),
    #[error("encode a cached {0}: {1}")]
    Json(&'static str, serde_json::Error),
    #[error("{0} is not a UTF-8 path")]
    Path(std::path::PathBuf),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The database: a writer and a read pool over one file.
#[derive(Clone, Debug)]
pub struct Store {
    writer: SqlitePool,
    reader: SqlitePool,
}

impl Store {
    /// Open (or create) the database at `path` and bring its schema up to
    /// date. Before pending migrations run on an existing database, it is
    /// copied beside itself as `<name>.before-<version>-<unix ms>`, the
    /// time being `now`: a copy per attempt, so one left by a migration
    /// that failed never blocks the next one made at a later time.
    pub async fn open(path: &Path, now: Timestamp) -> Result<Self> {
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
        migrate::run(&writer, path, now).await?;

        let reader = SqlitePoolOptions::new()
            .max_connections(READERS)
            .connect_with(options.read_only(true))
            .await?;
        Ok(Self { writer, reader })
    }

    /// A trivial query on the writer and on a reader, for readiness: the
    /// writer is the one connection every write waits for, and the readers
    /// serve every page. So it waits for the writer behind any write in
    /// progress.
    pub async fn ping(&self) -> Result<()> {
        for pool in [&self.writer, &self.reader] {
            sqlx::query!(r#"SELECT 1 AS "one!: i64""#)
                .fetch_one(pool)
                .await?;
        }
        Ok(())
    }

    /// Copy the database into a new file at `target`, compacted, as one
    /// read snapshot sees it: on a reader, so ingest and every other write
    /// go on meanwhile. `target` must not exist.
    pub async fn backup_into(&self, target: &Path) -> Result<()> {
        let path = target
            .to_str()
            .ok_or_else(|| Error::Path(target.to_owned()))?;
        sqlx::query!("VACUUM INTO ?", path)
            .execute(&self.reader)
            .await?;
        Ok(())
    }

    /// Wait for both pools to close, so the WAL is checkpointed.
    pub async fn close(&self) {
        self.reader.close().await;
        self.writer.close().await;
    }
}

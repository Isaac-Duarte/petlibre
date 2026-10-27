//! Installs: one row per install an owner asked for, rewritten as it
//! progresses and once it ends.

use jiff::Timestamp;
use wellfed_api::{FeederId, Hex, Install, InstallFailure, InstallId, InstallState, UserId};
use uuid::Uuid;

use crate::convert::{millis, timestamp};
use crate::{Error, Result, Store};

impl Store {
    /// Keep `install` as it stands now: a new row, or its row rewritten.
    pub async fn save_install(&self, install: &Install) -> Result<()> {
        let id = install.id.0;
        let feeder = install.feeder.0;
        let release = install.release.0.as_slice();
        let user = install.by.0;
        let started = millis(install.started);
        let state = install.state.code();
        let failure = install
            .failure
            .map(|failure| serde_json::to_string(&failure))
            .transpose()
            .map_err(|e| Error::Json("install failure", e))?;
        let finished = install.finished.map(millis);

        sqlx::query!(
            "INSERT INTO installs (id, feeder_id, release_id, version, user_id, started_ms,
                                   state, written, image_len, failure, finished_ms)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (id) DO UPDATE SET
                 state = excluded.state,
                 written = excluded.written,
                 failure = excluded.failure,
                 finished_ms = excluded.finished_ms",
            id,
            feeder,
            release,
            install.version,
            user,
            started,
            state,
            install.written,
            install.image_len,
            failure,
            finished,
        )
        .execute(&self.writer)
        .await?;
        Ok(())
    }

    /// The feeder's most recent install, running or not.
    pub async fn latest_install(&self, feeder: FeederId) -> Result<Option<Install>> {
        let feeder = feeder.0;
        let row = sqlx::query_as!(
            Raw,
            r#"SELECT id AS "id: Uuid", feeder_id AS "feeder_id: Uuid", release_id, version,
                      user_id AS "user_id: Uuid", started_ms, state, written, image_len,
                      failure, finished_ms
               FROM installs WHERE feeder_id = ?
               ORDER BY started_ms DESC, rowid DESC LIMIT 1"#,
            feeder
        )
        .fetch_optional(&self.reader)
        .await?;
        row.map(Install::try_from).transpose()
    }

    /// Mark every install still running as failed at `at`, interrupted:
    /// only the server that started one can run it, so at a start none is.
    /// How many there were.
    pub async fn interrupt_installs(&self, at: Timestamp) -> Result<u64> {
        let running = InstallState::Running.code();
        let failed = InstallState::Failed.code();
        let failure = serde_json::to_string(&InstallFailure::Interrupted)
            .map_err(|e| Error::Json("install failure", e))?;
        let at = millis(at);
        let done = sqlx::query!(
            "UPDATE installs SET state = ?, failure = ?, finished_ms = ? WHERE state = ?",
            failed,
            failure,
            at,
            running,
        )
        .execute(&self.writer)
        .await?;
        Ok(done.rows_affected())
    }
}

/// A row as SQLite holds it.
struct Raw {
    id: Uuid,
    feeder_id: Uuid,
    release_id: Vec<u8>,
    version: i64,
    user_id: Uuid,
    started_ms: i64,
    state: String,
    written: i64,
    image_len: i64,
    failure: Option<String>,
    finished_ms: Option<i64>,
}

impl TryFrom<Raw> for Install {
    type Error = Error;

    fn try_from(raw: Raw) -> Result<Self> {
        let small = |value: i64, what| u32::try_from(value).map_err(|_| Error::Corrupt(what));
        let failure = raw
            .failure
            .map(|json| serde_json::from_str(&json).map_err(|_| Error::Corrupt("install failure")))
            .transpose()?;

        Ok(Self {
            id: InstallId(raw.id),
            feeder: FeederId(raw.feeder_id),
            release: Hex(raw
                .release_id
                .try_into()
                .map_err(|_| Error::Corrupt("install release"))?),
            version: small(raw.version, "install version")?,
            by: UserId(raw.user_id),
            started: timestamp(raw.started_ms)?,
            state: InstallState::from_code(&raw.state).ok_or(Error::Corrupt("install state"))?,
            written: small(raw.written, "install progress")?,
            image_len: small(raw.image_len, "install length")?,
            failure,
            finished: raw.finished_ms.map(timestamp).transpose()?,
        })
    }
}

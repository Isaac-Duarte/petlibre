//! The server's own health: `GET /health`, liveness for anyone (a
//! container's or service manager's probe), and `GET /api/v1/status`,
//! readiness for an owner.
//!
//! Readiness is what an owner checks when something looks wrong: whether
//! the database answers, each feeder's link and the background jobs' last
//! runs. It is a view, not a metric endpoint: a household server has no
//! scraper to feed.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::FeederId;
use crate::feeders::Connection;

/// Liveness, without authentication.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Health {
    pub version: String,
}

/// Readiness, for an owner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ServerStatus {
    pub version: String,
    /// When this run of the server started.
    pub started: Timestamp,
    pub store: StoreState,
    /// Every adopted feeder and the server's session with it. Read from
    /// the database, so empty while `store` is `failing`.
    pub feeders: Vec<FeederLink>,
    /// Each background job's last run, once it has run, in the order the
    /// jobs are listed. Read from the database, so empty while `store` is
    /// `failing`.
    pub jobs: Vec<JobRun>,
}

/// Whether the database answered a trivial query on both its writer and
/// its readers. The failure itself is in the server's log, under the
/// request's id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StoreState {
    Ok,
    Failing,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FeederLink {
    pub id: FeederId,
    pub name: String,
    pub connection: Connection,
}

/// The server's background jobs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Job {
    /// Drops raw samples past the owner's retention.
    Retention,
    /// Catches the hourly sample rollups up.
    Rollups,
    /// Ends the sessions of expired accounts and of idle logins.
    Expiry,
    /// Copies the database into the data directory.
    Backup,
    /// Checks the project's releases, while the owner allows fetching.
    ReleaseCheck,
}

impl Job {
    /// Every job, in the order readiness lists them and the supervisor
    /// runs jobs that fall due at the same time.
    pub const ALL: [Self; 5] = [
        Self::Retention,
        Self::Rollups,
        Self::Expiry,
        Self::Backup,
        Self::ReleaseCheck,
    ];

    /// The stored spelling, the same as the JSON one.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Retention => "retention",
            Self::Rollups => "rollups",
            Self::Expiry => "expiry",
            Self::Backup => "backup",
            Self::ReleaseCheck => "release_check",
        }
    }

    /// The job a stored spelling names; `None` for one this build does not
    /// know.
    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|job| job.code() == code)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobOutcome {
    Succeeded,
    /// Why is in the server's log, under the job's name.
    Failed,
}

impl JobOutcome {
    /// The stored spelling, the same as the JSON one.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    /// The outcome a stored spelling names; `None` for one this build does
    /// not know.
    pub fn from_code(code: &str) -> Option<Self> {
        [Self::Succeeded, Self::Failed]
            .into_iter()
            .find(|outcome| outcome.code() == code)
    }
}

/// A job's most recent run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct JobRun {
    pub job: Job,
    pub started: Timestamp,
    pub finished: Timestamp,
    pub outcome: JobOutcome,
}

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
    /// Each background job's last run, once it has run.
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Job {
    /// Drops raw samples past the owner's retention.
    Retention,
    /// Catches the hourly sample rollups up.
    Rollups,
    /// Ends expired sessions and disables expired accounts.
    Expiry,
    /// Copies the database into the data directory.
    Backup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobOutcome {
    Succeeded,
    Failed,
}

/// A job's most recent run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct JobRun {
    pub job: Job,
    pub started: Timestamp,
    pub finished: Timestamp,
    pub outcome: JobOutcome,
}

//! What crosses the socket besides the live state and the job stream
//! (`wellfed_formats::bench::live`): the answer to a new job and the
//! refusals, which the client matches by variant.

use serde::{Deserialize, Serialize};
use wellfed_formats::bench::live::JobId;

/// Root-relative: the control socket, mode 0600. Only the owner of the
/// checkout can reach it, so it carries no token.
pub const SOCKET: &str = "private/bench.sock";

/// `POST /jobs` took the job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Accepted {
    pub id: JobId,
}

/// Why the server did not do what a client asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "kebab-case")]
pub enum Refusal {
    /// One job runs at a time, and a second is refused, never queued: a
    /// queued job would run on a bench nobody looked at since.
    #[error("The bench is busy with job {id} ({label}); wait for it, or cancel it first")]
    Busy { id: JobId, label: String },
    #[error("The bench server is stopping")]
    Stopping,
    #[error("No job {id} on this server")]
    NoSuchJob { id: JobId },
    #[error("Job {id} has already finished")]
    Finished { id: JobId },
    #[error("Session name {name:?} must be a single plain name")]
    BadName { name: String },
}

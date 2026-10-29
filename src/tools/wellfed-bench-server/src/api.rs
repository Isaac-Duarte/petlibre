//! What crosses the socket besides the live state and the job stream
//! (`wellfed_formats::bench::live`): a new job's command line, the answer
//! to it, and the refusals, which a client matches by variant.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use wellfed_formats::bench::live::JobId;

/// Root-relative: the control socket, mode 0600. Only the owner of the
/// checkout can reach it, so it carries no token.
pub const SOCKET: &str = "private/bench.sock";

/// `POST /jobs`: a job as the words of its command line, such as
/// `["dump", "unit-01"]`, without `--root`, and the client's checkout. The
/// server parses the words as the CLI does, so every client is held to the
/// same checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRequest {
    /// Canonical. A checkout whose `private/` is a link to another's
    /// reaches that one's server; the server refuses it, rather than run
    /// the other checkout's sources under this one's session names.
    pub root: PathBuf,
    pub argv: Vec<String>,
}

/// `POST /jobs` took the job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Accepted {
    pub id: JobId,
}

/// Why the server did not do what a client asked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "kebab-case")]
pub enum Refusal {
    /// One job runs at a time, and a second is refused, never queued: a
    /// queued job would run on a bench nobody looked at since.
    #[error("The bench is busy with job {id} ({label}); wait for it, or cancel it first")]
    Busy { id: JobId, label: String },
    #[error("The bench server is stopping")]
    Stopping,
    /// The server serves another checkout, whose `private/` this one's
    /// links to.
    #[error("The bench server runs for {server}, not {client}; run the command from that checkout")]
    OtherCheckout { server: String, client: String },
    #[error("No job {id} on this server")]
    NoSuchJob { id: JobId },
    #[error("Job {id} has already finished")]
    Finished { id: JobId },
    /// The words do not parse as a job; `message` is clap's, with usage.
    #[error("{message}")]
    Unparsed { message: String },
    /// A command that parses but that the client answers itself.
    #[error("`{what}` is answered by the client and is no job")]
    NotAJob { what: String },
    #[error("Session name {name:?} must be a single plain name")]
    BadName { name: String },
    #[error("--off-seconds {value} must be a finite, non-negative number, at most a week")]
    BadLength { value: f64 },
}

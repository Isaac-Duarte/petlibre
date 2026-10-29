//! The bench's read-only page: the bench server's live state, the feeder,
//! the integration suite's runs and the evidence sessions.
//!
//! `wellfed-bench serve --ui` serves it beside the bench server, which
//! hands it the live state and the jobs' output through [`Live`]. The crate
//! links no relay driver, no flash client and not the server itself, so
//! nothing here can switch a relay, open a serial port or start a job: it
//! cannot disturb an update, a dump or a capture, and nothing writes. The
//! views always build, so their tests and TypeScript bindings run in the
//! contributor gate; the server and its embedded web app need the `ui`
//! feature and a built `src/web/apps/bench/dist` (`make ui`).

#![forbid(unsafe_code)]

use std::sync::Arc;

use tokio::sync::watch;
use wellfed_formats::bench::live::{BenchState, JobId};

#[cfg(any(feature = "ui", test))]
mod feeder;
#[cfg(any(feature = "ui", test))]
mod runs;
#[cfg(any(feature = "ui", test))]
mod views;

#[cfg(feature = "ui")]
mod serve;
#[cfg(feature = "ui")]
pub use serve::{Listen, start};

#[cfg(test)]
mod bindings;

/// What only the bench server holds: its live state and its jobs' output,
/// both in memory, so the page follows a job without reading a file the
/// job is still writing through a buffer.
pub trait Live: Send + Sync {
    /// The state, woken on every change.
    fn subscribe(&self) -> watch::Receiver<BenchState>;
    /// Job `id`'s output, while the server keeps it.
    fn output(&self, id: JobId) -> Option<Arc<dyn JobOutput>>;
}

/// One job's output lines, as the server keeps them.
pub trait JobOutput: Send + Sync {
    /// The kept lines from line `cursor` on, and the cursor after them.
    fn read_from(&self, cursor: usize) -> (Vec<String>, usize);
    /// Woken on every line and at the job's end.
    fn subscribe(&self) -> watch::Receiver<u64>;
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("--listen needs a literal IP address")]
    Listen,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

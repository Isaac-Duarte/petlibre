//! The bench's page: the bench server's live state, the feeder, the
//! integration suite's runs and the evidence sessions, and the one thing it
//! sends back, the answer to a question a job asks (`install`'s prompts).
//!
//! `wellfed-bench serve --ui` serves it beside the bench server, which
//! hands it the live state, the jobs' output and the answer path through
//! [`Live`]. The crate links no relay driver, no flash client and not the
//! server itself, so nothing here can switch a relay, open a serial port
//! or start a job: it cannot disturb an update, a dump or a capture, and
//! the only thing it changes is a pending question, which the server
//! checks against its number and kind as it does any client's. The
//! views always build, so their tests and TypeScript bindings run in the
//! contributor gate; the server and its embedded web app need the `ui`
//! feature and a built `src/web/apps/bench/dist` (`make ui`).

#![forbid(unsafe_code)]

use std::sync::Arc;

use tokio::sync::watch;
use wellfed_formats::bench::live::{AnswerError, BenchState, JobId};
use wellfed_formats::bench::prompt::Answer;

#[cfg(any(feature = "ui", test))]
mod runs;
#[cfg(any(feature = "ui", test))]
mod stock;
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
    /// Answer job `id`'s question `seq`, as `POST /jobs/{id}/answer` on
    /// the server's socket does.
    fn answer(&self, id: JobId, seq: u32, answer: Answer) -> Result<(), AnswerError>;
}

/// One job's output lines, as the server keeps them.
pub trait JobOutput: Send + Sync {
    /// At most `most` kept lines from line `cursor` on, and the cursor
    /// after them: a page catching up copies a bounded batch at a time
    /// under the job's lock, never the whole log.
    fn read_from(&self, cursor: usize, most: usize) -> (Vec<String>, usize);
    /// The cursor after the last line, read without copying any.
    fn end(&self) -> usize;
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

//! The `report.json` every device session keeps beside its serial evidence.
//!
//! The report exists before the port opens and is rewritten after each
//! step, so a stop at any point leaves its latest state on disk. Each
//! command adds its own fields (`D`) to the lifecycle shared here; later
//! sessions read the result back by field name, as they read the reports
//! the former Python clients wrote.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::evidence::{self, utc};

pub use wellfed_formats::session::{Status, Transport};

/// Hashes of the finished serial streams, once a session closes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Streams {
    #[serde(
        rename = "serial-rx.bin_sha256",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub rx_sha256: Option<String>,
    #[serde(
        rename = "serial-tx.bin_sha256",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub tx_sha256: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Report<D> {
    pub status: Status,
    pub start_utc: String,
    #[serde(flatten)]
    pub details: D,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_utc: Option<String>,
    #[serde(flatten)]
    pub streams: Streams,
}

impl<D> Report<D> {
    /// Record a stop as `<kind>: <message>`, as the Python clients do.
    pub(crate) fn stop(&mut self, status: Status, error: &Error) {
        self.status = status;
        self.error = Some(format!("{}: {error}", error.kind()));
    }
}

/// A report and the file it is saved to after every change.
pub(crate) struct Session<D> {
    path: PathBuf,
    report: RefCell<Report<D>>,
}

impl<D: Serialize> Session<D> {
    /// Write the prepared report.
    pub(crate) fn open(path: PathBuf, details: D) -> Result<Self> {
        let session = Self {
            path,
            report: RefCell::new(Report {
                status: Status::Prepared,
                start_utc: utc(),
                details,
                error: None,
                end_utc: None,
                streams: Streams::default(),
            }),
        };
        session.save()?;
        Ok(session)
    }

    pub(crate) fn save(&self) -> Result<()> {
        evidence::save_report(&self.path, &*self.report.borrow())
    }

    /// Change the report and save it at once.
    pub(crate) fn update(&self, change: impl FnOnce(&mut Report<D>)) -> Result<()> {
        change(&mut self.report.borrow_mut());
        self.save()
    }

    /// Change the report in memory only; the next save writes it.
    pub(crate) fn edit(&self, change: impl FnOnce(&mut Report<D>)) {
        change(&mut self.report.borrow_mut());
    }

    pub(crate) fn read<T>(&self, read: impl FnOnce(&Report<D>) -> T) -> T {
        read(&self.report.borrow())
    }

    /// The last change, then the end time and stream hashes, saved once
    /// more. The report is returned even when that save fails.
    pub(crate) fn close(
        self,
        directory: &Path,
        change: impl FnOnce(&mut Report<D>),
    ) -> (Report<D>, Result<()>) {
        {
            let mut report = self.report.borrow_mut();
            change(&mut report);
            report.end_utc = Some(utc());
            report.streams = evidence::stream_hashes(directory);
        }

        let saved = self.save();
        (self.report.into_inner(), saved)
    }
}

/// The status a command's summary (stdout) or terminal report names; not a
/// session's lifecycle ([`Status`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryStatus {
    /// Refused before the port opened.
    Refused,
    /// A plan was written for review; nothing was sent.
    Planned,
    Prepared,
    Stopped,
    /// Every sector and the marker read back over the transports given.
    VerifiedViaSuppliedTransports,
}

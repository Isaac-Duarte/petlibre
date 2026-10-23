//! The `report.json` every device session keeps beside its serial evidence.
//!
//! The report exists before the port opens and is rewritten after each
//! step, so a stop at any point leaves its latest state on disk. Each
//! command adds its own fields (`D`) to the lifecycle shared here; later
//! sessions and the Python tools read the result back by field name.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::evidence::{self, utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Prepared,
    Running,
    /// A read or inspection finished.
    Complete,
    /// A read stopped (the Python reader's name for it).
    Failed,
    /// A write session stopped.
    Stopped,
    /// A write session's transaction verified.
    Verified,
}

/// How a session reached the helper. Evidence from one never continues the
/// other; Python reports name none and are serial.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    #[default]
    Serial,
    Emulator,
}

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
    pub fn stop(&mut self, status: Status, error: &Error) {
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
    pub fn open(path: PathBuf, details: D) -> Result<Self> {
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

    pub fn save(&self) -> Result<()> {
        evidence::save_report(&self.path, &*self.report.borrow())
    }

    /// Change the report and save it at once.
    pub fn update(&self, change: impl FnOnce(&mut Report<D>)) -> Result<()> {
        change(&mut self.report.borrow_mut());
        self.save()
    }

    /// Change the report in memory only; the next save writes it.
    pub fn edit(&self, change: impl FnOnce(&mut Report<D>)) {
        change(&mut self.report.borrow_mut());
    }

    pub fn read<T>(&self, read: impl FnOnce(&Report<D>) -> T) -> T {
        read(&self.report.borrow())
    }

    /// The last change, then the end time and stream hashes, saved once
    /// more. The report is returned even when that save fails.
    pub fn close(
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

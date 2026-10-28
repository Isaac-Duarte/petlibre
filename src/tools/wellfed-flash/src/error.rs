//! The crate's error type: every module's refusals, the image and format
//! errors of the crates below, I/O and JSON.
//!
//! A refusal, an I/O error and malformed JSON all end a session the same
//! way: terminally, with no retry, resume or fallback. Journals, stop reports
//! and the CLI's refusal summary record only the [`ErrorKind`] and the
//! message, so each variant's text is part of the recorded evidence and stays
//! exactly as the Python clients wrote it.

use std::fmt;
use std::io;

use wellfed_formats::bench::lease::LeaseError;

use crate::evidence::{journal, session};
use crate::link::{protocol, sector, status};
use crate::writers::{development, read, recovery, stock, update};

/// Every stop is terminal: nothing in this crate retries, resumes or falls
/// back after one of these.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Protocol(#[from] protocol::ProtocolError),
    #[error(transparent)]
    Sector(#[from] sector::SectorError),
    #[error(transparent)]
    Status(#[from] status::StatusError),
    #[error(transparent)]
    Session(#[from] session::SessionError),
    #[error(transparent)]
    Journal(#[from] journal::JournalError),
    #[error(transparent)]
    Read(#[from] read::ReadError),
    #[error(transparent)]
    Update(#[from] update::UpdateError),
    #[error(transparent)]
    Development(#[from] development::DevelopmentError),
    #[error(transparent)]
    Stock(#[from] stock::StockError),
    #[error(transparent)]
    Recovery(#[from] recovery::RecoveryError),
    /// The image, bank and review checks this client shares with the builder.
    #[error(transparent)]
    Image(#[from] wellfed_image::Error),
    /// Canonical JSON and private-file formats.
    #[error(transparent)]
    Formats(#[from] wellfed_formats::Error),
    /// Another tool holds the bench, or a killed run left its lease.
    #[error(transparent)]
    Lease(#[from] wellfed_formats::bench::lease::LeaseError),
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
}

/// The short kind of an [`Error`] that journals and stop reports record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ErrorKind {
    Refused,
    Io,
    Json,
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Refused => "refused",
            Self::Io => "io",
            Self::Json => "json",
        })
    }
}

impl Error {
    /// The kind a journal or report records: a wrapped crate's I/O and JSON
    /// errors keep their own kind, and everything else is a refusal.
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Io(_) => ErrorKind::Io,
            Self::Json(_) => ErrorKind::Json,

            Self::Image(wellfed_image::Error::Io(_)) => ErrorKind::Io,
            Self::Image(wellfed_image::Error::Json(_)) => ErrorKind::Json,
            Self::Formats(wellfed_formats::Error::Io(_)) => ErrorKind::Io,
            Self::Formats(wellfed_formats::Error::Json(_)) => ErrorKind::Json,
            Self::Lease(LeaseError::Io(_) | LeaseError::Formats(wellfed_formats::Error::Io(_))) => {
                ErrorKind::Io
            }

            Self::Image(
                wellfed_image::Error::Ota(_)
                | wellfed_image::Error::Bank(_)
                | wellfed_image::Error::Root(_)
                | wellfed_image::Error::Review(_),
            )
            | Self::Formats(wellfed_formats::Error::NotInteger)
            | Self::Lease(_)
            | Self::Protocol(_)
            | Self::Sector(_)
            | Self::Status(_)
            | Self::Session(_)
            | Self::Journal(_)
            | Self::Read(_)
            | Self::Update(_)
            | Self::Development(_)
            | Self::Stock(_)
            | Self::Recovery(_) => ErrorKind::Refused,
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

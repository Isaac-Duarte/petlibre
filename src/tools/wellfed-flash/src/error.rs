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

use crate::evidence::{journal, session};
use crate::link::{protocol, sector, status};
use crate::writers::{development, read, recovery, stock, update};

/// Every stop is terminal: nothing in this crate retries, resumes or falls
/// back after one of these.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Protocol(#[from] protocol::Error),
    #[error(transparent)]
    Sector(#[from] sector::Error),
    #[error(transparent)]
    Status(#[from] status::Error),
    #[error(transparent)]
    Session(#[from] session::Error),
    #[error(transparent)]
    Journal(#[from] journal::Error),
    #[error(transparent)]
    Read(#[from] read::Error),
    #[error(transparent)]
    Update(#[from] update::Error),
    #[error(transparent)]
    Development(#[from] development::Error),
    #[error(transparent)]
    Stock(#[from] stock::Error),
    #[error(transparent)]
    Recovery(#[from] recovery::Error),
    /// The image, bank and review checks this client shares with the builder.
    #[error(transparent)]
    Image(#[from] wellfed_image::Error),
    /// Canonical JSON and private-file formats.
    #[error(transparent)]
    Formats(#[from] wellfed_formats::Error),
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

            Self::Image(_)
            | Self::Formats(wellfed_formats::Error::NotInteger)
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

//! The crate's error type and `require`, the guard every check goes through.
//!
//! A refusal, an I/O error and malformed JSON all end a session the same
//! way: terminally, with no retry, resume or fallback.

use std::io;

/// Every stop is terminal: nothing in this crate retries, resumes or falls
/// back after one of these.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A pinned check, protocol expectation or evidence binding failed.
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
}

impl Error {
    /// The short kind recorded in journals and stop reports.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Refused(_) => "refused",
            Self::Io(_) => "io",
            Self::Json(_) => "json",
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Refuse with `message` unless `condition` holds.
pub fn require(condition: bool, message: impl Into<String>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(Error::Refused(message.into()))
    }
}

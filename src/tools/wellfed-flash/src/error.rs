//! The crate's error type and `require`, the guard every check goes through.
//!
//! A refusal, an I/O error and malformed JSON all end a session the same
//! way: terminally, with no retry, resume or fallback.

use std::fmt;
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

impl From<wellfed_formats::Error> for Error {
    fn from(error: wellfed_formats::Error) -> Self {
        match error {
            wellfed_formats::Error::NotInteger => Self::Refused(error.to_string()),
            wellfed_formats::Error::Io(error) => Self::Io(error),
            wellfed_formats::Error::Json(error) => Self::Json(error),
        }
    }
}

/// The image, bank and review checks keep their message and kind.
impl From<wellfed_image::Error> for Error {
    fn from(error: wellfed_image::Error) -> Self {
        match error {
            wellfed_image::Error::Refused(message) => Self::Refused(message),
            wellfed_image::Error::Io(error) => Self::Io(error),
            wellfed_image::Error::Json(error) => Self::Json(error),
        }
    }
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
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Refused(_) => ErrorKind::Refused,
            Self::Io(_) => ErrorKind::Io,
            Self::Json(_) => ErrorKind::Json,
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

/// Refuse with `message`: the unconditional form of [`require`].
pub fn refuse<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::Refused(message.into()))
}

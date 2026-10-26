//! The crate's error type and `require`, the guard every check goes through.
//!
//! The variants are the flash client's own, so a caller can carry a refusal
//! over with its message and kind unchanged.

use std::io;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A pinned check or evidence binding failed.
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

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Refuse with `message` unless `condition` holds.
pub(crate) fn require(condition: bool, message: impl Into<String>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(Error::Refused(message.into()))
    }
}

/// Refuse with `message`: the unconditional form of [`require`].
pub(crate) fn refuse<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::Refused(message.into()))
}

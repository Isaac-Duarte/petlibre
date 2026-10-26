//! The signer's error type and `require`, the guard every check goes through.
//!
//! A refusal is final: nothing here signs a fallback or retries.

use std::io;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A pinned check, key binding or review binding failed.
    #[error("{0}")]
    Refused(String),
    /// The frozen review or its image failed the checks the flash client
    /// makes before it installs one.
    #[error(transparent)]
    Review(#[from] wellfed_image::Error),
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

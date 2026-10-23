//! The builder's error type and `require`, the guard every check goes through.
//!
//! Every stop is terminal: the build never retries, and a failed build leaves
//! no manifest describing its outputs.

use std::io;
use std::path::PathBuf;
use std::process::ExitStatus;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A pinned check or input binding failed.
    #[error("{0}")]
    Refused(String),
    /// A tool could not be started.
    #[error("Cannot run {program}: {source}")]
    Spawn { program: String, source: io::Error },
    /// A tool ran and exited unsuccessfully.
    #[error("{program} failed ({status}): {stderr}")]
    Tool {
        program: String,
        status: ExitStatus,
        stderr: String,
    },
    /// A build stage failed; its full output stays in the private log.
    #[error("{stage}; see {}", log.display())]
    Logged { stage: &'static str, log: PathBuf },
    /// The offline stock-layout audit refused the reference or the candidate.
    #[error("Offline layout audit failed: {0}")]
    Layout(Box<Error>),
    /// The OTA container or region checks shared with the flash client.
    #[error(transparent)]
    Ota(#[from] wellfed_flash::Error),
    #[error(transparent)]
    Pattern(#[from] regex::Error),
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
pub fn require(condition: bool, message: impl Into<String>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(Error::Refused(message.into()))
    }
}

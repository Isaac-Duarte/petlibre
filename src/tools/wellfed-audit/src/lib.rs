//! Offline audits of PLAF103 evidence. None of them opens a serial port,
//! touches the network or writes flash; reports go to new private files.
//!
//! Ports of the Python auditors, with the same checks, outcomes and report
//! fields, so either version's reports read the same:
//! - `startup`: a receive-only startup capture against its source-bound build
//!   (`audit-firmware-startup.py`)
//! - `adc`: the power-input samples (`audit-firmware-adc.py`)
//! - `qa`: the QA firmware's guided self-test (new in Rust; no Python auditor)
//! - `transients`: logic-analyzer captures of the motor driver
//!   (`audit-driver-transients.py`)
//! - `backup`: two full-flash reads compared (`audit-flash-backup.py`)
//!
//! Each report is a typed struct whose fields are the report's keys; an
//! audit's sub-checks are flattened into it, so the JSON stays one object.

#![forbid(unsafe_code)]

pub mod adc;
pub mod backup;
pub(crate) mod capture;
pub mod qa;
pub mod startup;
pub mod transients;

#[cfg(test)]
mod test_support;

use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::Serialize;
use wellfed_formats::require;

pub use capture::CaptureError;

/// Why an audit refused its input. A refusal (exit status 2) means the audit
/// never ran, which is distinct from evidence that ran and failed. Each
/// audit's refusals keep their own enum, and every message is the operator's
/// one line, unchanged from the Python auditors where they had one.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Capture(#[from] CaptureError),
    #[error(transparent)]
    Startup(#[from] startup::StartupError),
    #[error(transparent)]
    Policy(#[from] startup::PolicyError),
    /// The ADC audit runs only on a build that declares an ADC policy.
    #[error("ADC policy required")]
    AdcPolicyRequired { declared: Option<String> },
    /// The QA audit runs only on the QA firmware.
    #[error("A QA firmware build required")]
    QaBuildRequired { firmware: Option<String> },
    #[error(transparent)]
    Transients(#[from] transients::TransientsError),
    #[error(transparent)]
    Backup(#[from] backup::BackupError),
    #[error(transparent)]
    Report(#[from] ReportError),
    /// The pinned stock OTA reference failed its container checks.
    #[error(transparent)]
    Reference(#[from] wellfed_image::Error),
    /// A manifest that failed to load or bind its source identity.
    #[error(transparent)]
    Formats(#[from] wellfed_formats::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// A report path refused: reports are new private files, never an overwrite
/// of an earlier report or of the evidence they describe.
#[derive(Debug, thiserror::Error)]
pub enum ReportError {
    #[error("Report already exists; choose a new path")]
    Exists { path: PathBuf },
    #[error("Report must be under ignored private/")]
    NotPrivate { path: PathBuf },
    #[error("Report cannot overwrite an input")]
    OverwritesInput { path: PathBuf },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub(crate) use wellfed_formats::canonical::sha256 as digest;

/// Python's `round(value, digits)`: the correctly rounded decimal, ties to even.
pub(crate) fn round(value: f64, digits: usize) -> f64 {
    format!("{value:.digits$}").parse().unwrap_or(value)
}

/// A closed set with fixed text: report outcomes, written as the Python
/// auditors' strings, and the names refusal messages interpolate.
macro_rules! outcomes {
    ($(#[$meta:meta])* $vis:vis enum $name:ident { $($variant:ident => $text:literal,)* }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        $vis enum $name {
            $($variant,)*
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)*
                }
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}
pub(crate) use outcomes;

/// A new private report under `root`'s ignored private/: never an
/// overwrite, a symlink target or one of the audit's `inputs`. Written once,
/// owner-only, and synced before the audit reports where it is.
pub fn write_report(
    root: &Path,
    path: &Path,
    report: &impl Serialize,
    inputs: &[&Path],
) -> Result<PathBuf> {
    require(fs::symlink_metadata(path).is_err(), || {
        ReportError::Exists { path: path.into() }
    })?;

    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)?;

    let resolved = parent
        .canonicalize()?
        .join(path.file_name().unwrap_or_default());
    require(
        resolved.starts_with(root.join("private").canonicalize()?),
        || ReportError::NotPrivate {
            path: resolved.clone(),
        },
    )?;
    require(
        !inputs
            .iter()
            .any(|input| input.canonicalize().is_ok_and(|p| p == resolved)),
        || ReportError::OverwritesInput {
            path: resolved.clone(),
        },
    )?;

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&resolved)?;
    file.write_all(format!("{}\n", serde_json::to_string_pretty(report)?).as_bytes())?;
    file.sync_all()?;
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_matches_python() {
        // Python: round(2.675, 2) == 2.67 (binary value just below the tie).
        assert_eq!(round(2.675, 2), 2.67);
        assert_eq!(round(0.5, 0), 0.0);
        assert_eq!(round(1.5, 0), 2.0);
        assert_eq!(round(100.0 / 3.0, 3), 33.333);
        assert_eq!(round(3277.0 * 1000.0 / 32768.0, 2), 100.01);
    }
}

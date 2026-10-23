//! Offline audits of PLAF103 evidence. None of them opens a serial port,
//! touches the network or writes flash; reports go to new private files.
//!
//! Ports of the Python auditors, with the same checks, outcomes and report
//! fields, so either version's reports read the same:
//! - `startup`: a receive-only startup capture against its source-bound build
//!   (`audit-firmware-startup.py`)
//! - `adc`: the FW14 power-input samples (`audit-firmware-adc.py`)
//! - `transients`: FW30 logic-analyzer captures of the motor driver
//!   (`audit-driver-transients.py`)
//! - `backup`: two full-flash reads compared (`audit-flash-backup.py`)

pub mod adc;
pub mod backup;
pub mod capture;
pub mod startup;
pub mod transients;

#[cfg(test)]
mod test_support;

use serde_json::{Map, Value};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A refused input: the audit cannot run on it.
    #[error("{0}")]
    Refused(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl From<petlibre_formats::Error> for Error {
    fn from(error: petlibre_formats::Error) -> Self {
        match error {
            petlibre_formats::Error::NotInteger => Self::Refused(error.to_string()),
            petlibre_formats::Error::Io(error) => Self::Io(error),
            petlibre_formats::Error::Json(error) => Self::Json(error),
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Refuse the input unless `condition` holds. A refusal (exit status 2) means
/// the audit never ran, which is distinct from evidence that ran and failed.
pub fn require(condition: bool, message: impl Into<String>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(Error::Refused(message.into()))
    }
}

pub use petlibre_formats::canonical::sha256 as digest;

/// A JSON object under construction, as the Python reports' dicts.
pub type Report = Map<String, Value>;

/// Python's `round(value, digits)`: the correctly rounded decimal, ties to even.
pub fn round(value: f64, digits: usize) -> f64 {
    format!("{value:.digits$}").parse().unwrap_or(value)
}

/// Copy every entry of `from` into `into` (Python's `dict.update`).
pub fn update(into: &mut Report, from: &Report) {
    for (key, value) in from {
        into.insert(key.clone(), value.clone());
    }
}

/// `dict.update` without the `outcome` key.
pub fn update_except_outcome(into: &mut Report, from: &Report) {
    for (key, value) in from {
        if key != "outcome" {
            into.insert(key.clone(), value.clone());
        }
    }
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

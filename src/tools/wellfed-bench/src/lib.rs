//! Relay bench for the PLAF103 research unit: the `wellfed-bench` binary
//! and its operator sequences.
//!
//! Relay 1 switches the feeder's USB 5V and relay 2 holds feeder TX to GND
//! through the 4.7 kΩ strap (`wellfed-relay`). Serial work is delegated
//! unchanged to the `wellfed-flash` binary, so its pinned checks, plan
//! hashes and no-retry rules apply. This crate never writes to the feeder
//! UART itself, never retries, and never boots after a stopped update. Ports
//! and evidence paths come from the ignored `private/bench.json`; see
//! docs/devices/plaf103/bench.md.
//!
//! The integration suite (`wellfed-itest`) runs on this crate's rig; the
//! read-only page (`wellfed-dashboard`) reads the progress this crate
//! records, and `--ui` serves it.

#![forbid(unsafe_code)]

/// `println!` that ignores a closed stdout. A reader such as `head` exiting
/// must not panic a sequence partway, past its power-off cleanup.
#[macro_export]
macro_rules! say {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

pub mod cli;
pub mod commands;
pub mod device;
pub mod server;
pub mod suite;
pub mod tools;

#[cfg(test)]
mod fingerprint;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Set by the operator's Ctrl-C; checked by every wait, so a sequence stops
/// (with its power-off cleanup) instead of running on.
pub static INTERRUPTED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A refusal or stop condition, reported to the operator as is.
    #[error("{0}")]
    Bench(String),
    #[error("Interrupted by the operator")]
    Interrupted,
    /// A JSON file the bench reads (its config, the server's, a review's
    /// manifest) is missing or malformed; `path` as the operator gave it.
    #[error("{}: {source}", path.display())]
    Load {
        path: PathBuf,
        #[source]
        source: wellfed_formats::Error,
    },
    /// Signing refused the review (it differs from what was frozen, or it
    /// moves without attendance) or the key; `review` root-relative.
    #[error("{}: {source}", review.display())]
    Signing {
        review: PathBuf,
        #[source]
        source: wellfed_build::Error,
    },
    /// The server's status does not show what a wait is for, yet.
    #[error(transparent)]
    NotReady(#[from] server::NotReady),
    /// The server does not yet see stock ready to be asked for an install.
    #[error(transparent)]
    StockNotReady(#[from] server::StockNotReady),
    /// A device API session could not be opened or did not answer.
    #[error(transparent)]
    Device(#[from] device::DeviceError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("Serial port: {0}")]
    Serial(#[from] serialport::Error),
    #[error(transparent)]
    Relay(#[from] wellfed_relay::Error),
    /// `wellfed-stock`'s loopback API refused or did not answer.
    #[error(transparent)]
    Api(#[from] wellfed_stock_api::ApiError),
    /// A progress or evidence file could not be written or read.
    #[error(transparent)]
    Formats(#[from] wellfed_formats::Error),
    /// The command line went to the wrong binary, or the suite's is missing
    /// or stale.
    #[error(transparent)]
    Route(#[from] cli::Refusal),
    /// The page could not take its address.
    #[cfg(feature = "ui")]
    #[error(transparent)]
    Page(#[from] wellfed_dashboard::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub fn require(condition: bool, message: impl Into<String>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(Error::Bench(message.into()))
    }
}

/// Read the JSON file at `relative` under `root` into its type.
pub fn load<T: serde::de::DeserializeOwned>(root: &Path, relative: &Path) -> Result<T> {
    wellfed_formats::load(&root.join(relative)).map_err(|source| Error::Load {
        path: relative.to_path_buf(),
        source,
    })
}

pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

/// Sleep in short slices so an interrupt ends the wait promptly.
pub fn sleep(seconds: f64) -> Result<()> {
    let end = Instant::now() + Duration::from_secs_f64(seconds.max(0.0));
    loop {
        if interrupted() {
            return Err(Error::Interrupted);
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(());
        }
        std::thread::sleep(left.min(Duration::from_millis(50)));
    }
}

pub(crate) use wellfed_formats::bench::now;

/// One hash over the sources both bench binaries are built from (`build.rs`):
/// `wellfed-itest` runs a hand-off only from a bench with the same one.
pub(crate) const BUILD_FINGERPRINT: &str = env!("WELLFED_BENCH_FINGERPRINT");

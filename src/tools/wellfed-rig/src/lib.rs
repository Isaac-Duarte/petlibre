//! The PLAF103 bench's rig: the operator sequences `wellfed-bench` runs,
//! and the power, capture, server and device API steps the integration
//! suite (`wellfed-itest`) builds its scenarios from.
//!
//! Relay 1 switches the feeder's USB 5V and relay 2 holds feeder TX to GND
//! through the 4.7 kΩ strap (`wellfed-relay`). Serial work is delegated
//! unchanged to the `wellfed-flash` binary, so its pinned checks, plan
//! hashes and no-retry rules apply. This crate never writes to the feeder
//! UART itself, never retries, and never boots after a stopped update. Ports
//! and evidence paths come from the ignored `private/bench.json`; see
//! docs/devices/plaf103/bench.md.
//!
//! The read-only page (`wellfed-dashboard`) reads the progress this crate
//! records, and `wellfed-bench --ui` serves it.

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

pub mod commands;
pub mod device;
pub mod server;
pub mod tools;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Set by the operator's Ctrl-C; checked by every wait, so a sequence stops
/// (with its power-off cleanup) instead of running on.
pub static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Everything a bench sequence stops on. A refusal or stop condition is a
/// variant of the module that makes it, whose `Display` is what the
/// operator reads and the progress record keeps; a lower crate's error is
/// wrapped whole.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A power, flash or capture sequence refused or stopped.
    #[error(transparent)]
    Command(#[from] commands::CommandsError),
    /// The over-the-air loop refused or stopped.
    #[error(transparent)]
    Ota(#[from] commands::ota::OtaError),
    /// A child tool or the device API could not do its part.
    #[error(transparent)]
    Tool(#[from] tools::ToolsError),
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
        source: wellfed_signing::Error,
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
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

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

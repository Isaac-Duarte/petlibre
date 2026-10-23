//! Relay bench for the PLAF103 research unit.
//!
//! Relay 1 switches the feeder's USB 5V and relay 2 holds feeder TX to GND
//! through the 4.7 kΩ strap. Serial work is delegated unchanged to the
//! `wellfed-flash` binary, so its pinned checks, plan hashes and no-retry
//! rules apply. This crate never writes to the feeder UART itself, never
//! retries, and never boots after a stopped update. Ports and evidence paths
//! come from the ignored `private/bench.json`; see docs/hardware/bench.md.

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
pub mod config;
mod local_api;
pub mod relay;
mod server;
pub mod status;
pub mod tools;
pub mod ui;

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
    /// The server's status does not show what a wait is for, yet.
    #[error(transparent)]
    NotReady(#[from] server::NotReady),
    /// The firmware did not answer a request. Requests go out at QoS 0, so
    /// one sent while the transport reconnects is lost.
    #[error("No reply to {op} within {seconds:.0} s")]
    NoReply { op: &'static str, seconds: f64 },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("Serial port: {0}")]
    Serial(#[from] serialport::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub(crate) fn require(condition: bool, message: impl Into<String>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(Error::Bench(message.into()))
    }
}

/// Read the JSON file at `relative` under `root` into its type.
pub(crate) fn load<T: serde::de::DeserializeOwned>(root: &Path, relative: &Path) -> Result<T> {
    wellfed_formats::load(&root.join(relative)).map_err(|source| Error::Load {
        path: relative.to_path_buf(),
        source,
    })
}

pub(crate) fn interrupted() -> bool {
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

/// Seconds since the Unix epoch, as the status files record times.
pub(crate) fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

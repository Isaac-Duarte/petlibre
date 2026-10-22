//! Relay bench for the PLAF103 research unit.
//!
//! Relay 1 switches the feeder's USB 5V and relay 2 holds feeder TX to GND
//! through the 4.7 kΩ strap. Serial work is delegated unchanged to the
//! `petlibre-flash` binary, so its pinned checks, plan hashes and no-retry
//! rules apply. This crate never writes to the feeder UART itself, never
//! retries, and never boots after a stopped update. Ports and evidence paths
//! come from the ignored `private/bench.json`; see docs/hardware/bench.md.

pub mod commands;
pub mod config;
pub mod relay;
pub mod status;
pub mod tools;
pub mod ui;

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
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("Serial port: {0}")]
    Serial(#[from] serialport::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub fn require(condition: bool, message: impl Into<String>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(Error::Bench(message.into()))
    }
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

pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Seconds since the Unix epoch, as the status files record times.
pub fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

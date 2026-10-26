//! What the bench reads from and sends to `wellfed-stock`: its status
//! document (`GET /api/status`) and the stock trial bodies. The server
//! relays no requests (they go over the device API, `crate::device`) and
//! hears nothing from the replacement firmware, which speaks no MQTT.
//!
//! The document's type is `wellfed-stock`'s own (`wellfed_formats::stock`),
//! and strict where the server is: a trial record with a field or phase the
//! bench does not know fails the whole read, which stops the wait that
//! needed it rather than guessing. Both are built from this repository, so
//! a mismatch means rebuild one.

use std::fmt::Display;
use std::ops::Deref;

use wellfed_model::WifiSource;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A `wellfed-stock` status document.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(transparent)]
pub struct StockStatus(pub wellfed_formats::stock::StockStatus);

impl Deref for StockStatus {
    type Target = wellfed_formats::stock::StockStatus;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub use wellfed_formats::stock::{TrialAnswer, TrialRequest};

/// `POST /api/ota/trial/resolve`: close a failed or uncertain trial from the
/// server's own evidence; and `POST /api/ota/trial/answered`: close it on
/// the replacement's device-API answer ([`TrialAnswer`]), which the note
/// describes. Neither sends the device anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrialResolve<'a> {
    pub request_id: &'a str,
    pub note: &'a str,
}

/// Why the feeder's device-API status does not show what a wait is for, yet.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NotReady {
    /// The first 12 digits of the build the device API reports.
    #[error("the feeder reports build {0}")]
    Running(String),
    #[error("the feeder runs from bank {0}")]
    OnBank(u8),
    #[error("the feeder's clock is not set")]
    ClockUnset,
    #[error("the firmware clock is {0} ms behind this host")]
    Skew(i64),
    #[error("Wi-Fi source {0:?}")]
    Source(WifiSource),
}

/// The only stock release a replacement install starts from.
pub const STOCK_VERSION: &str = "1.1.15";

/// Why the server's stock view does not allow an install yet.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StockNotReady {
    #[error("no current stock telemetry (is the feeder on the hotspot and routed?)")]
    Offline,
    #[error("stock reports version {}, not {STOCK_VERSION}", shown(.0))]
    Version(Option<String>),
    #[error("no stock version report yet")]
    NoVersionReport,
    #[error("stock reports power type {} (1 is the adapter)", shown(.0))]
    Power(Option<u64>),
    #[error("serve was started without --enable-ota-trial")]
    TrialDisabled,
}

fn shown<T: Display>(value: &Option<T>) -> String {
    value
        .as_ref()
        .map_or_else(|| "?".into(), ToString::to_string)
}

impl StockStatus {
    /// Stock 1.1.15 is online, freshly reported and on its adapter: what the
    /// server's trial gate also requires before asking it to install.
    pub fn stock_ready(&self) -> Result<String, StockNotReady> {
        if !self.device_online {
            return Err(StockNotReady::Offline);
        }
        let version = self
            .attributes
            .get("softwareVersion")
            .and_then(Value::as_str);
        if version != Some(STOCK_VERSION) {
            return Err(StockNotReady::Version(version.map(str::to_string)));
        }
        if self.version_reported_at.is_none() {
            return Err(StockNotReady::NoVersionReport);
        }
        let power = self.attributes.get("powerType").and_then(Value::as_u64);
        if power != Some(1) {
            return Err(StockNotReady::Power(power));
        }
        if !self.ota_trial_enabled {
            return Err(StockNotReady::TrialDisabled);
        }
        Ok(format!("Stock {STOCK_VERSION} online on external power"))
    }
}

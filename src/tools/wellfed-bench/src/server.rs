//! What the bench reads from and sends to `wellfed-local`: its status
//! document (`GET /api/status`) and the stock trial bodies. The
//! server relays no requests (they go over the device API, `crate::device`)
//! and only bank 0's recovery role reports health over MQTT.
//!
//! The document's type is `wellfed-local`'s own (`wellfed_formats::local`),
//! and strict where the server is: a trial record with a field or phase the
//! bench does not know fails the whole read, which stops the wait that
//! needed it rather than guessing. Both are built from this repository, so
//! a mismatch means rebuild one.
//!
//! Health inside it is the firmware's own JSON, relayed by the server
//! unchanged and typed as far as the bench checks it
//! ([`wellfed_formats::bench::health`], which the page shares).

use std::fmt::Display;
use std::ops::Deref;

use wellfed_formats::bench::health::Health;
use wellfed_formats::local::{Replacement, TrialRecord};
use wellfed_model::WifiSource;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A `wellfed-local` status document, with the replacement's health read
/// as far as the bench checks it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(transparent)]
pub struct LocalStatus(pub wellfed_formats::local::LocalStatus<Health>);

impl Deref for LocalStatus {
    type Target = wellfed_formats::local::LocalStatus<Health>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// `POST /api/ota/trial` for the replacement image: the server sends stock
/// one `OTA_UPGRADE` for it and never resends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrialRequest {
    pub request_id: String,
    /// The image server's lease token.
    pub server_token: String,
}

/// `POST /api/ota/trial/resolve`: close a failed or uncertain trial from the
/// server's own evidence; sends the device nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrialResolve<'a> {
    pub request_id: &'a str,
    pub note: &'a str,
}

/// Why a status document does not show what a wait is for, yet.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NotReady {
    #[error("the server has no replacement-firmware role")]
    NoRole,
    #[error("no health received yet")]
    NoHealth,
    #[error("health reports the firmware offline")]
    Offline,
    #[error("health is stale (age {} ms)", shown(.0))]
    Stale(Option<i64>),
    /// The first 12 digits of the build health reports.
    #[error("health reports build {0}")]
    Build(String),
    #[error("health reports bank {}", shown(.0))]
    Bank(Option<u64>),
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

impl LocalStatus {
    pub(crate) fn replacement(&self) -> Result<&Replacement<Health>, NotReady> {
        self.replacement.as_ref().ok_or(NotReady::NoRole)
    }

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

    /// The trial with `request_id`.
    pub fn trial(&self, request_id: &str) -> Option<&TrialRecord> {
        self.ota_trials
            .iter()
            .find(|trial| trial.input.request_id.to_string() == request_id)
    }

    /// The health the server holds, when it is online and fresh.
    pub fn fresh_health(&self) -> Result<&Health, NotReady> {
        let replacement = self.replacement()?;
        let health = replacement.health.as_ref().ok_or(NotReady::NoHealth)?;
        if !replacement.online {
            return Err(NotReady::Offline);
        }
        if replacement.stale {
            return Err(NotReady::Stale(replacement.age_ms));
        }
        Ok(health)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NEW: &str = "8a13e59efa15e56b5f5e32825909dc45e381a2027abc65d8de6658c40b302447";
    #[test]
    fn a_document_without_freshness_is_stale() {
        let status: LocalStatus = serde_json::from_value(
            json!({"replacement": {"health": {"build": NEW}, "online": true}}),
        )
        .unwrap();
        assert_eq!(status.fresh_health().unwrap_err(), NotReady::Stale(None));
    }
}

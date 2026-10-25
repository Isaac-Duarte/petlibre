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

use wellfed_formats::bench::health::{Health, HealthRole};
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
    /// Bank 0 is the recovery role, which must say so.
    #[error("health from bank 0 is not the recovery role's")]
    NotRecovery,
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
    pub fn replacement(&self) -> Result<&Replacement<Health>, NotReady> {
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

    /// The bank-0 recovery role's health from `build`, while it waits for an
    /// update: what a build stock just installed reports.
    pub fn recovery_health(&self, build: &str) -> Result<String, NotReady> {
        let report = self.health_on(build, 0)?;
        match self.fresh_health()?.role {
            Some(HealthRole::Recovery) => Ok(report + ", recovery role"),
            None => Err(NotReady::NotRecovery),
        }
    }

    /// Whether fresh health shows `build` (lowercase) running on `bank`: a
    /// one-line report of it. Bank 0 after stock installed the build, since
    /// stock writes the bank it is not running from.
    pub fn health_on(&self, build: &str, bank: u64) -> Result<String, NotReady> {
        let health = self.fresh_health()?;

        let reported = health.build.to_ascii_lowercase();
        if reported != build {
            let short = reported.get(..12).filter(|s| !s.is_empty());
            return Err(NotReady::Build(short.unwrap_or("none").into()));
        }
        if health.bank != Some(bank) {
            return Err(NotReady::Bank(health.bank));
        }

        let faults = if health.faults.is_empty() {
            "none".into()
        } else {
            health.faults.join(", ")
        };
        let mut report = format!(
            "Health: build {} on bank {bank}, seq {}, outputs {}, faults {faults}",
            &build[..build.len().min(12)],
            shown(&health.seq),
            health.outputs.as_deref().unwrap_or("?"),
        );
        if let Some(clock) = &health.clock {
            let valid = if clock.unix_ms.is_some() {
                "valid"
            } else {
                "not set"
            };
            report += &format!(", clock {valid} (syncs {})", clock.syncs);
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NEW: &str = "8a13e59efa15e56b5f5e32825909dc45e381a2027abc65d8de6658c40b302447";
    const OLD: &str = "5b8433a4f795cb2178c7a38a45087a6727e7a2621fc557d146cff607556f991e";

    fn status(health: Value, online: bool, stale: bool) -> LocalStatus {
        serde_json::from_value(json!({"replacement": {
            "health": health, "online": online, "stale": stale, "age_ms": 5}}))
        .unwrap()
    }

    #[test]
    fn health_must_show_the_build_on_bank_1_online_and_fresh() {
        let good = json!({"build": NEW.to_uppercase(), "bank": 1, "seq": 3, "outputs": "sensing",
                          "faults": ["watchdog", "flash_hook"], "clock": {"syncs": 0}});
        assert_eq!(
            status(good.clone(), true, false).health_on(NEW, 1).unwrap(),
            "Health: build 8a13e59efa15 on bank 1, seq 3, outputs sensing, faults watchdog, flash_hook, clock not set (syncs 0)"
        );

        let mut bank0 = good.clone();
        bank0["bank"] = json!(0);
        let mut old = good.clone();
        old["build"] = json!(OLD);
        let unconfigured: LocalStatus =
            serde_json::from_value(json!({"replacement": null})).unwrap();
        for (status, reason) in [
            (unconfigured, NotReady::NoRole),
            (status(Value::Null, false, true), NotReady::NoHealth),
            (
                status(old, true, false),
                NotReady::Build("5b8433a4f795".into()),
            ),
            (status(bank0, true, false), NotReady::Bank(Some(0))),
            (status(good.clone(), false, false), NotReady::Offline),
            (status(good, true, true), NotReady::Stale(Some(5))),
        ] {
            assert_eq!(status.health_on(NEW, 1).unwrap_err(), reason);
        }
        assert_eq!(
            NotReady::Stale(Some(5)).to_string(),
            "health is stale (age 5 ms)"
        );
    }
}

//! `wellfed-local`'s status document (`GET /api/status`), which the bench
//! reads to follow stock and its install trials.
//!
//! The replacement firmware's health inside it is the firmware's own JSON,
//! which the server relays as sent: each side types it as it needs (`H`),
//! the server keeping every field and the bench only what it checks. The
//! trial records are the server's persisted ledger entries, so they refuse
//! fields they do not know; the rest of the document defaults what is
//! absent.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

/// The server's view of stock and of the replacement firmware.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalStatus<H> {
    /// The feeder's MQTT session reaches the server's broker.
    pub broker_connected: bool,
    /// Stock telemetry is current.
    pub device_online: bool,
    pub last_seen: Option<DateTime<Utc>>,
    pub clock_offset_seconds: Option<i64>,
    /// Stock's reported attributes (`softwareVersion`, `powerType`, …), as
    /// stock sent them.
    pub attributes: Map<String, Value>,
    /// When stock last reported its version.
    pub version_reported_at: Option<DateTime<Utc>>,
    pub power_reported_at: Option<DateTime<Utc>>,
    pub build_commit: String,
    pub build_clean: bool,
    /// Stock's own OTA reports, oldest first.
    pub ota_reports: Vec<OtaReport>,
    pub ota_trial_enabled: bool,
    /// Newest first.
    pub ota_trials: Vec<TrialRecord>,
    /// `None` when the server has no replacement-firmware role.
    pub replacement: Option<Replacement<H>>,
}

impl<H> Default for LocalStatus<H> {
    fn default() -> Self {
        Self {
            broker_connected: false,
            device_online: false,
            last_seen: None,
            clock_offset_seconds: None,
            attributes: Map::new(),
            version_reported_at: None,
            power_reported_at: None,
            build_commit: String::new(),
            build_clean: false,
            ota_reports: Vec::new(),
            ota_trial_enabled: false,
            ota_trials: Vec::new(),
            replacement: None,
        }
    }
}

/// The replacement firmware's latest health, as the server holds it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Replacement<H> {
    pub health: Option<H>,
    /// Each transport session's first health gives a new one.
    pub received_at: Option<DateTime<Utc>>,
    pub age_ms: Option<i64>,
    pub online: bool,
    pub stale: bool,
    /// Health messages the server refused.
    pub rejected: u64,
}

/// No health, or a document without `stale`, is never fresh.
impl<H> Default for Replacement<H> {
    fn default() -> Self {
        Self {
            health: None,
            received_at: None,
            age_ms: None,
            online: false,
            stale: true,
            rejected: 0,
        }
    }
}

/// A stock `OTA_UPGRADE`, `OTA_PROGRESS` or `OTA_INFORM` report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OtaReport {
    /// Tells repeated reports apart.
    pub received_at: DateTime<Utc>,
    pub command: String,
    pub message_id: Option<String>,
    pub phase: Option<String>,
    pub state: Option<u8>,
    pub progress: Option<u8>,
    pub code: Option<i64>,
    pub error_code: Option<u32>,
}

impl fmt::Display for OtaReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "stock {}", self.command)?;
        if let Some(phase) = &self.phase {
            write!(f, " phase {phase}")?;
        }
        if let Some(progress) = self.progress {
            write!(f, " {progress}%")?;
        }
        match self.error_code {
            Some(code) if code != 0 => write!(f, " error 0x{code:x}"),
            _ => Ok(()),
        }
    }
}

/// One stock-to-replacement install trial in the server's ledger.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrialRecord {
    pub sequence: usize,
    pub input: TrialInput,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub phase: TrialPhase,
    pub download_reported: bool,
    pub resolution_note: Option<String>,
    /// The digest the command announced, from the image server's lease.
    pub image_sha256: String,
}

/// What a trial was asked with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrialInput {
    pub request_id: Uuid,
    /// The image server's lease token.
    pub server_token: Uuid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrialPhase {
    Submitted,
    VersionReported,
    ReportedError,
    Uncertain,
    /// Operator-resolved after a quiet period with the source version still
    /// freshly reported. Never sends a command.
    NotInstalled,
    /// A replacement install whose firmware then published online health on
    /// its own role. Stock reports nothing once it has activated the
    /// replacement, so this, not a version report, closes the trial.
    ReplacementHealthy,
}

/// `wellfed-local`'s config, root-relative: the suite serves with it and the
/// page reads the server's address and token from it.
pub const SERVER_CONFIG: &str = "private/server.json";

/// The parts of `wellfed-local`'s config the bench tools use.
#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub api_token: String,
    pub http_listen: std::net::SocketAddr,
    pub mqtt_listen: std::net::SocketAddr,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_document_without_freshness_is_stale() {
        let status: LocalStatus<Value> =
            serde_json::from_value(json!({"replacement": {"health": {}, "online": true}})).unwrap();
        assert!(status.replacement.unwrap().stale);
    }

    #[test]
    fn a_trial_record_refuses_fields_it_does_not_know() {
        let id = Uuid::nil();
        let record = json!({"sequence": 1, "input": {"request_id": id, "server_token": id},
                            "created_at": "2026-10-25T00:00:00Z",
                            "updated_at": "2026-10-25T00:00:00Z", "phase": "submitted",
                            "download_reported": false, "resolution_note": null,
                            "image_sha256": "00"});
        let read: TrialRecord = serde_json::from_value(record.clone()).unwrap();
        assert_eq!(read.phase, TrialPhase::Submitted);

        let mut extra = record;
        extra["retries"] = json!(1);
        assert!(serde_json::from_value::<TrialRecord>(extra).is_err());
    }
}

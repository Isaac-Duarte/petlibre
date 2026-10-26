//! `wellfed-stock`'s status document (`GET /api/status`), which the bench
//! reads to follow stock and its install trials.
//!
//! The trial records are the server's persisted ledger entries, so they
//! refuse fields they do not know; the rest of the document defaults what
//! is absent and ignores what it does not know.

use std::fmt;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

/// The server's view of stock.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StockStatus {
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
}

/// Which stock OTA message a report came in, by its `cmd`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OtaCommand {
    #[serde(rename = "OTA_UPGRADE")]
    Upgrade,
    #[serde(rename = "OTA_PROGRESS")]
    Progress,
    #[serde(rename = "OTA_INFORM")]
    Inform,
}

impl OtaCommand {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Upgrade => "OTA_UPGRADE",
            Self::Progress => "OTA_PROGRESS",
            Self::Inform => "OTA_INFORM",
        }
    }

    /// The command a stock message's `cmd` names, if it is an OTA report.
    pub fn from_cmd(cmd: &str) -> Option<Self> {
        [Self::Upgrade, Self::Progress, Self::Inform]
            .into_iter()
            .find(|command| command.as_str() == cmd)
    }
}

/// A stock `OTA_UPGRADE`, `OTA_PROGRESS` or `OTA_INFORM` report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OtaReport {
    /// Tells repeated reports apart.
    pub received_at: DateTime<Utc>,
    pub command: OtaCommand,
    pub message_id: Option<String>,
    pub phase: Option<String>,
    pub state: Option<u8>,
    pub progress: Option<u8>,
    pub code: Option<i64>,
    pub error_code: Option<u32>,
}

impl fmt::Display for OtaReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "stock {}", self.command.as_str())?;
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
    /// The served build's identity (its frozen review's startup identity,
    /// 64 lowercase hex), from the same lease. A trial closes on the
    /// replacement's answer only when that answer names this build. `None`
    /// in records written before it was kept: those never close that way.
    #[serde(default)]
    pub build: Option<String>,
    /// Stock reported its version after the trial was submitted, so it was
    /// still running then. A trial so marked never closes on a client's
    /// word that the replacement answered: only fresh version evidence
    /// resolves it.
    #[serde(default)]
    pub stock_reported: bool,
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
    /// A replacement install whose build then answered the device API from
    /// bank 0, as the operator's client checked and reported it. Stock
    /// reports nothing once it has activated the replacement, so this, not
    /// a version report, closes the trial.
    ReplacementHealthy,
}

/// `wellfed-stock`'s config, root-relative: the suite serves with it and the
/// page reads the server's address and token from it.
pub const SERVER_CONFIG: &str = "private/server.json";

/// The parts of `wellfed-stock`'s config the bench tools use.
#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub api_token: String,
    pub http_listen: std::net::SocketAddr,
    pub mqtt_listen: std::net::SocketAddr,
}

/// The image server's lease file, beside the server config.
pub const IMAGE_LEASE_FILE: &str = "ota-image-server.json";

/// Private record of the running image server (`wellfed-stock ota-images`),
/// written beside the config. Trial dispatch requires its token, an
/// unexpired accept window and a live listener; the bench reads the token
/// for its trial request.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImageLease {
    pub token: Uuid,
    pub peer: Ipv4Addr,
    pub expires_at: DateTime<Utc>,
    /// The served build's SHA-256, which the install command announces.
    pub sha256: String,
    /// The served build's identity: its frozen manifest's startup identity,
    /// 64 lowercase hex, as the build's device API reports it.
    pub build: String,
}

impl ImageLease {
    /// The lease file beside `config_path`.
    pub fn path(config_path: &Path) -> PathBuf {
        config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(IMAGE_LEASE_FILE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ota_commands_keep_stock_s_words() {
        for command in [
            OtaCommand::Upgrade,
            OtaCommand::Progress,
            OtaCommand::Inform,
        ] {
            assert_eq!(serde_json::to_value(command).unwrap(), command.as_str());
            assert_eq!(OtaCommand::from_cmd(command.as_str()), Some(command));
        }
        assert_eq!(OtaCommand::from_cmd("OTA_START"), None);
    }

    #[test]
    fn a_document_from_an_older_server_still_reads() {
        // Servers before the replacement's broker was deleted also sent its
        // health; the field is ignored.
        let status: StockStatus =
            serde_json::from_value(json!({"device_online": true, "replacement": null})).unwrap();
        assert!(status.device_online);
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

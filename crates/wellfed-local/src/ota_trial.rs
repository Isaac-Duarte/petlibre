//! Fixed modified-stock/original trial requests; no arbitrary firmware or URLs.
//!
//! Each `Image` names one hash-pinned file on the `ota-images` server and the
//! stock version it must start from. A restore (`Original`, `CProbeOriginal`)
//! is allowed only after its experiment's version was reported. The controller
//! persists a `Record` before the `OTA_UPGRADE` command goes out and never
//! resends it.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{
    config::Config,
    ota_image,
    protocol::{self, Reply},
};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Image {
    Test,
    Original,
    CProbe,
    CProbeOriginal,
}

impl Image {
    pub fn source_version(self) -> &'static str {
        match self {
            Self::Test | Self::CProbe => "1.1.15",
            Self::Original => "1.1.16",
            Self::CProbeOriginal => "1.1.17",
        }
    }

    pub fn target_version(self) -> &'static str {
        match self {
            Self::Test => "1.1.16",
            Self::CProbe => "1.1.17",
            Self::Original | Self::CProbeOriginal => "1.1.15",
        }
    }

    fn filename(self) -> &'static str {
        match self {
            Self::Test => "test.bin",
            Self::CProbe => "probe.bin",
            Self::Original | Self::CProbeOriginal => "original.bin",
        }
    }

    fn digest(self) -> &'static str {
        match self {
            Self::Test => ota_image::TEST_SHA256,
            Self::CProbe => ota_image::C_PROBE_SHA256,
            Self::Original | Self::CProbeOriginal => ota_image::ORIGINAL_SHA256,
        }
    }

    pub fn profile(self) -> ota_image::Profile {
        match self {
            Self::Test | Self::Original => ota_image::Profile::StockVersion,
            Self::CProbe | Self::CProbeOriginal => ota_image::Profile::CExecution,
        }
    }

    pub fn required_installation(self) -> Option<Self> {
        match self {
            Self::Original => Some(Self::Test),
            Self::CProbeOriginal => Some(Self::CProbe),
            Self::Test | Self::CProbe => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub request_id: Uuid,
    pub image: Image,
    pub server_token: Uuid,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Submitted,
    VersionReported,
    ReportedError,
    Uncertain,
    /// Operator-resolved after a quiet period with the source version still
    /// freshly reported. Never sends a command.
    NotInstalled,
}

/// Explicit operator resolution of an uncertain or failed trial.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveInput {
    pub request_id: Uuid,
    pub note: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Record {
    pub sequence: usize,
    pub input: Input,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub phase: Phase,
    #[serde(default)]
    pub download_reported: bool,
    #[serde(default)]
    pub resolution_note: Option<String>,
}

impl Record {
    pub fn command(&self, config: &Config) -> Reply {
        let mut command = protocol::service(
            config,
            "OTA_UPGRADE",
            self.input.request_id,
            self.created_at,
        );

        command.payload["upgradeType"] = json!(1);
        command.payload["targetPid"] = json!("PLAF103");
        command.payload["targetSoftwareVersion"] = json!(self.input.image.target_version());
        command.payload["sha256"] = json!(self.input.image.digest());
        command.payload["url"] = json!(format!(
            "http://{}:{}/ota/{}/{}",
            config.mqtt_listen.ip(),
            ota_image::PORT,
            self.input.server_token,
            self.input.image.filename()
        ));

        command
    }
}

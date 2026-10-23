//! Fixed modified-stock/original trial requests; no arbitrary firmware or URLs.
//!
//! Each `Image` names one hash-pinned file on the `ota-images` server and the
//! stock version it must start from. A restore (`Original`, `CProbeOriginal`)
//! is allowed only after its experiment's version was reported. The controller
//! persists a `Record` before the `OTA_UPGRADE` command goes out and never
//! resends it.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    config::Config,
    ota_image,
    protocol::{self, Channel, Reply},
};

/// The original stock application, the only one feeding is allowed under.
pub const ORIGINAL_VERSION: &str = "1.1.15";
/// The modified-stock version-string test image.
pub const TEST_VERSION: &str = "1.1.16";
/// The C-probe diagnostic image.
pub const C_PROBE_VERSION: &str = "1.1.17";

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
            Self::Test | Self::CProbe => ORIGINAL_VERSION,
            Self::Original => TEST_VERSION,
            Self::CProbeOriginal => C_PROBE_VERSION,
        }
    }

    pub fn target_version(self) -> &'static str {
        match self {
            Self::Test => TEST_VERSION,
            Self::CProbe => C_PROBE_VERSION,
            Self::Original | Self::CProbeOriginal => ORIGINAL_VERSION,
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

/// The stock `OTA_UPGRADE` command's own fields.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Upgrade {
    sha256: &'static str,
    target_pid: &'static str,
    target_software_version: &'static str,
    upgrade_type: u8,
    url: String,
}

impl Record {
    pub fn command(&self, config: &Config) -> Reply {
        let image = self.input.image;
        let upgrade = Upgrade {
            sha256: image.digest(),
            target_pid: "PLAF103",
            target_software_version: image.target_version(),
            upgrade_type: 1,
            url: format!(
                "http://{}:{}/ota/{}/{}",
                config.mqtt_listen.ip(),
                ota_image::PORT,
                self.input.server_token,
                image.filename()
            ),
        };

        protocol::command(
            config,
            Channel::Service,
            "OTA_UPGRADE",
            self.input.request_id,
            self.created_at,
            &upgrade,
        )
    }
}

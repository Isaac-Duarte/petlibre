//! The stock-to-replacement install: one `OTA_UPGRADE` naming the build
//! the `ota-images` server holds, by the digest its frozen review binds
//! (carried in the server's lease and kept in the record). No arbitrary
//! URLs. The controller persists a `Record` before the command goes out and
//! never resends it.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    config::Config,
    ota::image,
    protocol::{self, Channel, Reply},
};

/// The stock application an install starts from.
pub const ORIGINAL_VERSION: &str = "1.1.15";
/// What a replacement build is announced as. Stock parses it only with
/// `%d.%d.%d` and refuses its own version; nothing reports it back.
pub const REPLACEMENT_VERSION: &str = "2.0.0";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub request_id: Uuid,
    /// The image server's lease token.
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
    /// A replacement install whose firmware then published online health on
    /// its own role. Stock reports nothing once it has activated the
    /// replacement, so this, not a version report, closes the trial.
    ReplacementHealthy,
}

/// Explicit operator resolution of an uncertain or failed trial.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveInput {
    pub request_id: Uuid,
    pub note: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub sequence: usize,
    pub input: Input,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub phase: Phase,
    pub download_reported: bool,
    pub resolution_note: Option<String>,
    /// The digest the command announced, from the image server's lease.
    pub image_sha256: String,
}

/// The stock `OTA_UPGRADE` command's own fields.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Upgrade<'a> {
    sha256: &'a str,
    target_pid: &'static str,
    target_software_version: &'static str,
    upgrade_type: u8,
    url: String,
}

impl Record {
    pub fn command(&self, config: &Config) -> Reply {
        let upgrade = Upgrade {
            sha256: &self.image_sha256,
            target_pid: "PLAF103",
            target_software_version: REPLACEMENT_VERSION,
            upgrade_type: 1,
            url: format!(
                "http://{}:{}/ota/{}/{}",
                config.mqtt_listen.ip(),
                image::PORT,
                self.input.server_token,
                image::ROUTE
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

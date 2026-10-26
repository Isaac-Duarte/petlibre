//! The stock-to-replacement install: one `OTA_UPGRADE` naming the build
//! the `ota-images` server holds, by the digest its frozen review binds
//! (carried in the server's lease and kept in the record). No arbitrary
//! URLs. The controller persists a `Record` before the command goes out and
//! never resends it.
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

pub use wellfed_formats::stock::{
    TrialInput as Input, TrialPhase as Phase, TrialRecord as Record,
};

/// Explicit operator resolution of an uncertain or failed trial.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveInput {
    pub request_id: Uuid,
    pub note: String,
}

/// What the operator's client saw: the trial's build answering its device
/// API from `bank` (which must be 0) after the submission. `build` is the
/// identity the API reported, 64 lowercase hex; the note is free text.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AnsweredInput {
    pub request_id: Uuid,
    pub build: String,
    pub bank: u8,
    pub note: String,
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

/// The one `OTA_UPGRADE` a trial sends.
pub fn command(record: &Record, config: &Config) -> Reply {
    let upgrade = Upgrade {
        sha256: &record.image_sha256,
        target_pid: "PLAF103",
        target_software_version: REPLACEMENT_VERSION,
        upgrade_type: 1,
        url: format!(
            "http://{}:{}/ota/{}/{}",
            config.mqtt_listen.ip(),
            image::PORT,
            record.input.server_token,
            image::ROUTE
        ),
    };

    protocol::command(
        config,
        Channel::Service,
        "OTA_UPGRADE",
        record.input.request_id,
        record.created_at,
        &upgrade,
    )
}

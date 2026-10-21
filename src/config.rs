use std::{fs, net::SocketAddr, path::Path};

use anyhow::{Context, Result, bail, ensure};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::storage;

// Intentionally no Debug: this structure contains working device credentials.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub device_id: String,
    pub device_client_id: String,
    pub device_username: String,
    pub device_password: String,
    pub server_password: String,
    pub api_token: String,
    pub mqtt_listen: SocketAddr,
    pub http_listen: SocketAddr,
    pub timezone: Tz,
    pub max_portions: u8,
    pub plans: Vec<Value>,
    pub binding_response: Option<Value>,
    pub hotspot_interface: String,
    pub target_mac: String,
    /// Replacement-firmware MQTT role (FW08); absent for stock-only setups.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware: Option<FirmwareConfig>,
}

// Intentionally no Debug: holds the replacement firmware's broker password.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FirmwareConfig {
    pub client_id: String,
    pub username: String,
    pub password: String,
}

pub const FIRMWARE_USERNAME: &str = "petlibre-fw";
/// Topic-layout version of the replacement-firmware namespace.
pub const FIRMWARE_TOPIC_VERSION: u32 = 1;

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let value: Self = serde_json::from_slice(&fs::read(path).context("read configuration")?)
            .context("invalid server configuration")?;
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "unsupported configuration version");
        ensure!(
            !self.device_id.is_empty()
                && self.device_id.len() <= 64
                && self
                    .device_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "invalid device identifier"
        );
        ensure!(
            self.max_portions > 0 && self.max_portions <= 10,
            "max_portions must be 1..10"
        );
        ensure!(
            self.http_listen.ip().is_loopback(),
            "HTTP API must bind to loopback"
        );
        ensure!(
            self.mqtt_listen.is_ipv4() && !self.mqtt_listen.ip().is_unspecified(),
            "MQTT must bind to a specific IPv4 address"
        );
        ensure!(
            self.api_token.len() >= 32 && self.server_password.len() >= 32,
            "server secrets must have at least 32 characters"
        );
        ensure!(
            !self.device_password.is_empty() && !self.device_username.is_empty(),
            "device credentials missing"
        );
        ensure!(
            self.hotspot_interface
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                && !self.hotspot_interface.is_empty(),
            "invalid hotspot interface"
        );
        ensure!(
            self.target_mac.split(':').count() == 6
                && self
                    .target_mac
                    .split(':')
                    .all(|part| part.len() == 2 && u8::from_str_radix(part, 16).is_ok()),
            "invalid target MAC"
        );
        if let Some(firmware) = &self.firmware {
            ensure!(
                firmware.client_id == self.firmware_client_id()
                    && firmware.username == FIRMWARE_USERNAME,
                "invalid replacement-firmware identity"
            );
            ensure!(
                firmware.password.len() == 64
                    && firmware.password.bytes().all(|b| b.is_ascii_hexdigit()),
                "replacement-firmware password must be 64 hex characters"
            );
        }
        Ok(())
    }

    pub fn topic(&self, suffix: &str) -> String {
        format!("dl/PLAF103/{}/device/{suffix}", self.device_id)
    }

    pub fn server_client_id(&self) -> String {
        format!("local-server-{}", self.device_id)
    }

    pub fn firmware_client_id(&self) -> String {
        format!("petlibre-fw-{}", self.device_id)
    }

    /// The replacement firmware's namespace, with a trailing slash.
    pub fn firmware_namespace(&self) -> String {
        format!("petlibre/{FIRMWARE_TOPIC_VERSION}/{}/", self.device_id)
    }

    pub fn firmware_topic(&self, suffix: &str) -> String {
        self.firmware_namespace() + suffix
    }
}

/// The private file the replacement-firmware build reads (FW08).
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FirmwareCredential {
    pub version: u32,
    pub broker: std::net::Ipv4Addr,
    pub port: u16,
    pub device_id: String,
    pub client_id: String,
    pub username: String,
    pub password: String,
    pub namespace: String,
}

/// Add the replacement-firmware role to a stopped server's configuration and
/// write the firmware's private credential file. Refuses an existing role or
/// output file, so a credential never rotates silently. The output is written
/// first: if the configuration update then fails, delete it and rerun.
pub fn add_firmware_credential(config_path: &Path, output: &Path) -> Result<()> {
    let mut config = Config::load(config_path)?;
    ensure!(
        config.firmware.is_none(),
        "configuration already has a replacement-firmware credential; refusing to rotate it"
    );
    ensure!(
        !output.exists(),
        "credential file already exists; refusing to replace it"
    );
    let std::net::IpAddr::V4(broker) = config.mqtt_listen.ip() else {
        bail!("MQTT must bind to a specific IPv4 address");
    };
    // Same source as the other server secrets: 2 x 122 random bits.
    let password = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let firmware = FirmwareConfig {
        client_id: config.firmware_client_id(),
        username: FIRMWARE_USERNAME.to_owned(),
        password,
    };
    let credential = FirmwareCredential {
        version: 1,
        broker,
        port: config.mqtt_listen.port(),
        device_id: config.device_id.clone(),
        client_id: firmware.client_id.clone(),
        username: firmware.username.clone(),
        password: firmware.password.clone(),
        namespace: config.firmware_namespace(),
    };
    config.firmware = Some(firmware);
    config.validate()?;
    storage::write_new_json(output, &credential)?;
    storage::write_json(config_path, &config)
}

/// Contributor-specific settings that cannot be inferred from a packet capture.
pub struct ImportOptions {
    pub mqtt_listen: SocketAddr,
    pub hotspot_interface: String,
    pub timezone: Tz,
}

pub fn import(
    credentials: &Path,
    capture: &Path,
    output: &Path,
    options: ImportOptions,
) -> Result<()> {
    ensure!(
        !output.exists(),
        "configuration already exists; refusing to replace secrets"
    );
    let credentials: Vec<Value> = serde_json::from_slice(&fs::read(credentials)?)?;
    ensure!(
        credentials.len() == 1,
        "expected exactly one captured CONNECT record"
    );
    let get = |field: &str| -> Result<String> {
        Ok(credentials[0][field]
            .as_str()
            .context("CONNECT credential field missing")?
            .to_owned())
    };
    let packets: Vec<Value> = serde_json::from_slice(&fs::read(capture)?)?;
    let mut device_id = None;
    let mut plans = None;
    let mut binding_response = None;
    let mut target_mac = None;
    for packet in packets {
        let mqtt = &packet["_source"]["layers"]["mqtt"];
        let Some(topic) = mqtt["mqtt.topic"].as_str() else {
            continue;
        };
        let parts: Vec<_> = topic.split('/').collect();
        if parts.len() != 6 || parts[0] != "dl" || parts[1] != "PLAF103" {
            continue;
        }
        if device_id.as_deref().is_some_and(|id| id != parts[2]) {
            bail!("capture contains multiple feeder identifiers");
        }
        device_id = Some(parts[2].to_owned());
        if parts[5] == "post"
            && let Some(mac) = packet["_source"]["layers"]["eth"]["eth.src"].as_str()
        {
            ensure!(
                target_mac.as_deref().is_none_or(|previous| previous == mac),
                "capture contains multiple device MAC addresses"
            );
            target_mac = Some(mac.to_owned());
        }
        let Some(hex) = mqtt["mqtt.msg"].as_str() else {
            continue;
        };
        let compact = hex.replace(':', "");
        if compact.len() % 2 != 0 || !compact.is_ascii() {
            continue;
        }
        let bytes: Result<Vec<u8>, _> = (0..compact.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&compact[i..i + 2], 16))
            .collect();
        let Ok(bytes) = bytes else { continue };
        let Ok(message) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        if parts[5] == "sub"
            && matches!(
                message["cmd"].as_str(),
                Some("GET_FEEDING_PLAN_EVENT" | "FEEDING_PLAN_SERVICE")
            )
            && let Some(values) = message["plans"].as_array()
        {
            plans = Some(values.clone());
        }
        if parts[4] == "system" && parts[5] == "sub" && message["cmd"] == "BINDING" {
            binding_response = Some(message);
        }
    }
    let config = Config {
        version: 1,
        device_id: device_id.context("capture has no PLAF103 topics")?,
        device_client_id: get("mqtt.clientid")?,
        device_username: get("mqtt.username")?,
        device_password: get("mqtt.passwd")?,
        server_password: Uuid::new_v4().simple().to_string(),
        api_token: Uuid::new_v4().simple().to_string(),
        mqtt_listen: options.mqtt_listen,
        http_listen: "127.0.0.1:8765".parse()?,
        timezone: options.timezone,
        max_portions: 1,
        plans: plans.context("capture has no authoritative feeding-plan snapshot")?,
        binding_response,
        hotspot_interface: options.hotspot_interface,
        target_mac: target_mac.context("capture lacks a device source MAC")?,
        firmware: None,
    };
    config.validate()?;
    storage::write_json(output, &config)
}

//! The private server configuration (`private/server.json`, format version 1):
//! stock's part (`wellfed_stock::config`) and the replacement firmware's
//! broker role beside it.
//!
//! The role is the file's `firmware` field, which stock's own type refuses,
//! so loading splits it off before reading the rest as stock's, and writing
//! puts it back after stock's fields. The file keeps one shape either way. It
//! holds working credentials, so it is written mode 0600 and has no `Debug`.
//! Edit it only while the service is stopped.

use std::{
    fs,
    ops::{Deref, DerefMut},
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};
use wellfed_formats::credentials::FirmwareCredential;
use wellfed_stock::storage;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

pub use wellfed_stock::config::{Config as StockConfig, ImportOptions, import};

// Intentionally no Debug: this structure contains working device credentials.
#[derive(Clone, Serialize)]
pub struct Config {
    #[serde(flatten)]
    pub stock: StockConfig,
    /// Replacement-firmware MQTT role; absent for stock-only setups.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub firmware: Option<FirmwareConfig>,
}

/// The role extends stock's configuration rather than wrapping it: every
/// stock field reads as before, and the extension goes with the role.
impl Deref for Config {
    type Target = StockConfig;

    fn deref(&self) -> &StockConfig {
        &self.stock
    }
}

impl DerefMut for Config {
    fn deref_mut(&mut self) -> &mut StockConfig {
        &mut self.stock
    }
}

// Intentionally no Debug: holds the replacement firmware's broker password.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FirmwareConfig {
    pub client_id: String,
    pub username: String,
    pub password: String,
    /// The request-signing key. Unused since requests moved
    /// from MQTT to the Noise device API; kept only so existing
    /// configurations still parse (`deny_unknown_fields`), and removed with
    /// the broker credentials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_key: Option<String>,
}

pub const FIRMWARE_USERNAME: &str = "wellfed-fw";
/// Topic-layout version of the replacement-firmware namespace.
pub const FIRMWARE_TOPIC_VERSION: u32 = 1;

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).context("read configuration")?;
        let value = Self::parse(&bytes).context("invalid server configuration")?;
        value.validate()?;
        Ok(value)
    }

    /// The role's field off the top, then the rest as stock's, which still
    /// refuses any field it does not know. A repeated key is refused, as a
    /// struct parse of the whole file would.
    pub(crate) fn parse(bytes: &[u8]) -> serde_json::Result<Self> {
        let UniqueKeys(mut fields) = serde_json::from_slice(bytes)?;
        let firmware = fields
            .remove("firmware")
            .map(serde_json::from_value::<Option<FirmwareConfig>>)
            .transpose()?
            .flatten();
        let stock = serde_json::from_value(Value::Object(fields))?;
        Ok(Self { stock, firmware })
    }

    pub fn validate(&self) -> Result<()> {
        self.stock.validate()?;

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

    pub fn firmware_client_id(&self) -> String {
        format!("wellfed-fw-{}", self.device_id)
    }

    /// The replacement firmware's namespace, with a trailing slash.
    pub fn firmware_namespace(&self) -> String {
        format!("wellfed/{FIRMWARE_TOPIC_VERSION}/{}/", self.device_id)
    }

    pub fn firmware_topic(&self, suffix: &str) -> String {
        self.firmware_namespace() + suffix
    }
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
        request_key: None,
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
        request_key: None,
    };

    config.firmware = Some(firmware);
    config.validate()?;
    storage::write_new_json(output, &credential)?;
    storage::write_json(config_path, &config)
}

/// A JSON object whose keys each appear once.
struct UniqueKeys(Map<String, Value>);

impl<'de> Deserialize<'de> for UniqueKeys {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueKeys;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON object")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut access: A,
            ) -> Result<UniqueKeys, A::Error> {
                let mut fields = Map::new();
                while let Some((key, value)) = access.next_entry::<String, Value>()? {
                    if fields.contains_key(&key) {
                        return Err(serde::de::Error::custom(format!("duplicate field `{key}`")));
                    }
                    fields.insert(key, value);
                }
                Ok(UniqueKeys(fields))
            }
        }

        deserializer.deserialize_map(Visitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STOCK: &str = r#""version":1,"device_id":"AF_TEST","device_client_id":"AF_TEST",
        "device_username":"dev","device_password":"pw",
        "server_password":"ssssssssssssssssssssssssssssssss",
        "api_token":"tttttttttttttttttttttttttttttttt",
        "mqtt_listen":"127.0.0.1:1883","http_listen":"127.0.0.1:8765",
        "timezone":"UTC","plans":[],"binding_response":null,
        "hotspot_interface":"wlan1","target_mac":"02:00:00:00:00:01""#;

    const FIRMWARE: &str = r#""firmware":{"client_id":"wellfed-fw-AF_TEST",
        "username":"wellfed-fw",
        "password":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;

    #[test]
    fn a_repeated_key_is_refused_as_the_struct_parse_did() {
        let once = format!("{{{STOCK},{FIRMWARE}}}");
        let config = Config::parse(once.as_bytes()).unwrap();
        assert!(config.firmware.is_some() && config.validate().is_ok());

        let firmware_twice = format!("{{{STOCK},{FIRMWARE},{FIRMWARE}}}");
        let error = Config::parse(firmware_twice.as_bytes()).err().unwrap();
        assert!(error.to_string().contains("duplicate field `firmware`"));

        let version_twice = format!("{{{STOCK},\"version\":1}}");
        let error = Config::parse(version_twice.as_bytes()).err().unwrap();
        assert!(error.to_string().contains("duplicate field `version`"));
    }
}

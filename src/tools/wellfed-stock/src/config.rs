//! The stock server's private configuration (`private/server.json`, format
//! version 1): stock's captured device credentials and plans, and the
//! server's own secrets and addresses.
//!
//! `init` imports it once from a captured CONNECT credential and a decoded
//! MQTT capture, generating independent API and server secrets, and refuses
//! to overwrite an existing file. It holds working device credentials, so it
//! is written mode 0600 and has no `Debug`. Edit it only while the service is
//! stopped. `validate` keeps the HTTP API on loopback and the broker on one
//! specific IPv4 address.
//!
//! A file written while the replacement firmware still had a broker role
//! keeps that role's `firmware` field. [`load`] drops it unread, so such a
//! file still loads without being rewritten (it holds working credentials
//! and is edited only by hand, with the service stopped); any other field
//! the type does not know is still refused.

use std::{fs, net::SocketAddr, path::Path};

use anyhow::{Context, Result, bail, ensure};
use chrono_tz::Tz;
use wellfed_formats::hex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::{protocol::lenient, storage};

/// The retired replacement-firmware broker role's field.
const RETIRED_ROLE: &str = "firmware";

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
    pub plans: Vec<Value>,
    /// The captured cloud `BINDING` reply, replayed verbatim.
    pub binding_response: Option<Map<String, Value>>,

    pub hotspot_interface: String,
    pub target_mac: String,
}

impl Config {
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

        Ok(())
    }

    pub fn topic(&self, suffix: &str) -> String {
        format!("dl/PLAF103/{}/device/{suffix}", self.device_id)
    }

    pub fn server_client_id(&self) -> String {
        format!("local-server-{}", self.device_id)
    }
}

/// Contributor-specific settings that cannot be inferred from a packet capture.
pub struct ImportOptions {
    pub mqtt_listen: SocketAddr,
    pub hotspot_interface: String,
    pub timezone: Tz,
}

/// One captured CONNECT record, as the capture tooling exports it.
#[derive(Deserialize)]
struct ConnectRecord {
    #[serde(rename = "mqtt.clientid")]
    client_id: String,
    #[serde(rename = "mqtt.username")]
    username: String,
    #[serde(rename = "mqtt.passwd")]
    password: String,
}

/// The parts of a `tshark -T json` packet the import reads. Every layer is
/// optional and lenient: a frame of another shape (tshark writes an array for
/// several MQTT messages in one frame) is skipped, not an error.
#[derive(Deserialize)]
struct Packet {
    #[serde(rename = "_source", default, deserialize_with = "lenient")]
    source: Option<Source>,
}

#[derive(Deserialize)]
struct Source {
    layers: Layers,
}

#[derive(Deserialize)]
struct Layers {
    #[serde(default, deserialize_with = "lenient")]
    mqtt: Option<MqttLayer>,
    #[serde(default, deserialize_with = "lenient")]
    eth: Option<EthLayer>,
}

#[derive(Deserialize)]
struct MqttLayer {
    #[serde(rename = "mqtt.topic", default, deserialize_with = "lenient")]
    topic: Option<String>,
    /// The payload as colon-separated (or plain) hex.
    #[serde(rename = "mqtt.msg", default, deserialize_with = "lenient")]
    msg: Option<String>,
}

#[derive(Deserialize)]
struct EthLayer {
    #[serde(rename = "eth.src", default, deserialize_with = "lenient")]
    src: Option<String>,
}

/// A captured message the import keeps: the plan snapshot or the binding.
#[derive(Deserialize)]
struct Captured {
    #[serde(default, deserialize_with = "lenient")]
    cmd: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    plans: Option<Vec<Value>>,
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

    let credentials: Vec<ConnectRecord> = serde_json::from_slice(&fs::read(credentials)?)
        .context("CONNECT credential fields missing")?;
    let [credential] = <[ConnectRecord; 1]>::try_from(credentials)
        .ok()
        .context("expected exactly one captured CONNECT record")?;

    let packets: Vec<Packet> = serde_json::from_slice(&fs::read(capture)?)?;
    let mut device_id = None;
    let mut plans = None;
    let mut binding_response = None;
    let mut target_mac = None;

    for packet in packets {
        let Some(layers) = packet.source.map(|source| source.layers) else {
            continue;
        };
        let Some(mqtt) = layers.mqtt else {
            continue;
        };
        let Some(topic) = mqtt.topic else {
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
            && let Some(mac) = layers.eth.and_then(|eth| eth.src)
        {
            ensure!(
                target_mac.as_deref().is_none_or(|previous| previous == mac),
                "capture contains multiple device MAC addresses"
            );
            target_mac = Some(mac);
        }

        let Some(bytes) = mqtt
            .msg
            .and_then(|text| hex::decode_vec(&text.replace(':', "")))
        else {
            continue;
        };
        let Ok(message) = serde_json::from_slice::<Map<String, Value>>(&bytes) else {
            continue;
        };
        let Ok(captured) = Captured::deserialize(&message) else {
            continue;
        };

        if parts[5] == "sub"
            && matches!(
                captured.cmd.as_deref(),
                Some("GET_FEEDING_PLAN_EVENT" | "FEEDING_PLAN_SERVICE")
            )
            && let Some(values) = captured.plans
        {
            plans = Some(values);
        }

        if parts[4] == "system" && parts[5] == "sub" && captured.cmd.as_deref() == Some("BINDING") {
            binding_response = Some(message);
        }
    }

    let config = Config {
        version: 1,
        device_id: device_id.context("capture has no PLAF103 topics")?,
        device_client_id: credential.client_id,
        device_username: credential.username,
        device_password: credential.password,
        server_password: Uuid::new_v4().simple().to_string(),
        api_token: Uuid::new_v4().simple().to_string(),
        mqtt_listen: options.mqtt_listen,
        http_listen: "127.0.0.1:8765".parse()?,
        timezone: options.timezone,
        plans: plans.context("capture has no authoritative feeding-plan snapshot")?,
        binding_response,
        hotspot_interface: options.hotspot_interface,
        target_mac: target_mac.context("capture lacks a device source MAC")?,
    };

    config.validate()?;
    storage::write_json(output, &config)
}

pub fn load(path: &Path) -> Result<Config> {
    let bytes = fs::read(path).context("read configuration")?;
    let config = parse(&bytes).context("invalid server configuration")?;
    config.validate()?;
    Ok(config)
}

/// The retired role's field off the top, then the rest as stock's. A
/// repeated key is refused, as a struct parse of the whole file would.
pub(crate) fn parse(bytes: &[u8]) -> serde_json::Result<Config> {
    let UniqueKeys(mut fields) = serde_json::from_slice(bytes)?;
    fields.remove(RETIRED_ROLE);
    serde_json::from_value(Value::Object(fields))
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
        "username":"wellfed-fw","request_key":null,
        "password":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;

    #[test]
    fn a_file_with_the_retired_role_still_loads() {
        let config = parse(format!("{{{STOCK},{FIRMWARE}}}").as_bytes()).unwrap();
        assert!(config.validate().is_ok());
        assert_eq!(config.device_id, "AF_TEST");
    }

    #[test]
    fn a_repeated_key_is_refused_as_the_struct_parse_did() {
        let firmware_twice = format!("{{{STOCK},{FIRMWARE},{FIRMWARE}}}");
        let error = parse(firmware_twice.as_bytes()).err().unwrap();
        assert!(error.to_string().contains("duplicate field `firmware`"));

        let version_twice = format!("{{{STOCK},\"version\":1}}");
        let error = parse(version_twice.as_bytes()).err().unwrap();
        assert!(error.to_string().contains("duplicate field `version`"));
    }

    #[test]
    fn any_other_unknown_field_is_still_refused() {
        let extra = format!("{{{STOCK},\"other\":1}}");
        assert!(parse(extra.as_bytes()).is_err());
    }
}

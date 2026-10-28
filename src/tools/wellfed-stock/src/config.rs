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
//!
//! An installer's config ([`InstallConfig`], written by `init-installer`)
//! has no device at all: the broker trusts the first stock feeder that
//! connects during an install ([`crate::enroll`]), and [`load`] then takes
//! the device from that enrollment beside the config. A file names its
//! device one way or the other, never both.

use std::{
    fs,
    net::{IpAddr, SocketAddr},
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;
use wellfed_formats::hex;

use crate::{enroll::Enrollment, protocol::lenient, storage};

/// The retired replacement-firmware broker role's field.
const RETIRED_ROLE: &str = "firmware";
/// The field a config with a captured device has, and an installer's lacks.
const DEVICE_FIELD: &str = "device_id";

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
    #[serde(default)]
    pub plans: Vec<Value>,
    /// The captured cloud `BINDING` reply, replayed verbatim.
    #[serde(default)]
    pub binding_response: Option<Map<String, Value>>,

    /// The bench hotspot the feeder joins, and the feeder's MAC on it: what
    /// the `network` redirect and neighbour discovery need. An installer's
    /// config has neither.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotspot_interface: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_mac: Option<String>,
    /// Where an enrolled feeder first connected from, which the image
    /// server admits when no MAC is known. Filled from the enrollment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_address: Option<IpAddr>,
}

/// Stock's device ID: what its topics carry, and (physical, the captured
/// CONNECT) its MQTT client ID too.
pub fn valid_device_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The server's own checks, shared by both kinds of config.
fn validate_server(
    version: u32,
    http_listen: SocketAddr,
    mqtt_listen: SocketAddr,
    api_token: &str,
    server_password: &str,
) -> Result<()> {
    ensure!(version == 1, "unsupported configuration version");
    ensure!(
        http_listen.ip().is_loopback(),
        "HTTP API must bind to loopback"
    );
    ensure!(
        mqtt_listen.is_ipv4() && !mqtt_listen.ip().is_unspecified(),
        "MQTT must bind to a specific IPv4 address"
    );
    ensure!(
        api_token.len() >= 32 && server_password.len() >= 32,
        "server secrets must have at least 32 characters"
    );
    Ok(())
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        validate_server(
            self.version,
            self.http_listen,
            self.mqtt_listen,
            &self.api_token,
            &self.server_password,
        )?;
        ensure!(
            valid_device_id(&self.device_id),
            "invalid device identifier"
        );
        ensure!(
            !self.device_password.is_empty() && !self.device_username.is_empty(),
            "device credentials missing"
        );

        if let Some(interface) = &self.hotspot_interface {
            ensure!(
                interface
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                    && !interface.is_empty(),
                "invalid hotspot interface"
            );
        }
        if let Some(mac) = &self.target_mac {
            ensure!(
                mac.split(':').count() == 6
                    && mac
                        .split(':')
                        .all(|part| part.len() == 2 && u8::from_str_radix(part, 16).is_ok()),
                "invalid target MAC"
            );
        }

        Ok(())
    }

    /// The hotspot interface and the feeder's MAC on it, which only a
    /// bench config has.
    pub fn hotspot(&self) -> Result<(&str, &str)> {
        match (&self.hotspot_interface, &self.target_mac) {
            (Some(interface), Some(mac)) => Ok((interface, mac)),
            _ => bail!("this config names no hotspot interface and feeder MAC (an installer's)"),
        }
    }

    pub fn topic(&self, suffix: &str) -> String {
        format!("dl/PLAF103/{}/device/{suffix}", self.device_id)
    }

    pub fn server_client_id(&self) -> String {
        server_client_id(&self.device_id)
    }
}

/// The server's own MQTT client ID for `device_id`'s session.
pub fn server_client_id(device_id: &str) -> String {
    format!("local-server-{device_id}")
}

/// An installer's config: the server's secrets and addresses, and no
/// device until one enrolls. Like [`Config`], no `Debug`.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallConfig {
    pub version: u32,
    pub server_password: String,
    pub api_token: String,
    pub mqtt_listen: SocketAddr,
    pub http_listen: SocketAddr,
    pub timezone: Tz,
}

impl InstallConfig {
    pub fn validate(&self) -> Result<()> {
        validate_server(
            self.version,
            self.http_listen,
            self.mqtt_listen,
            &self.api_token,
            &self.server_password,
        )
    }
}

/// A new installer's config at `output`, with fresh secrets; never replaces
/// a file.
pub fn init_installer(output: &Path, mqtt_listen: SocketAddr, timezone: Tz) -> Result<()> {
    let config = InstallConfig {
        version: 1,
        server_password: Uuid::new_v4().simple().to_string(),
        api_token: Uuid::new_v4().simple().to_string(),
        mqtt_listen,
        http_listen: "127.0.0.1:8765".parse()?,
        timezone,
    };
    config.validate()?;
    storage::write_new_json(output, &config)
}

/// What a config file names as its device.
pub enum State {
    /// A captured device, or an installer's with its enrollment.
    Device(Box<Config>),
    /// An installer's config before any feeder enrolled.
    Unenrolled(InstallConfig),
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
        hotspot_interface: Some(options.hotspot_interface),
        target_mac: Some(target_mac.context("capture lacks a device source MAC")?),
        device_address: None,
    };

    config.validate()?;
    storage::write_json(output, &config)
}

/// The config at `path` with its device: captured, or enrolled beside it.
pub fn load(path: &Path) -> Result<Config> {
    match state(path)? {
        State::Device(config) => Ok(*config),
        State::Unenrolled(_) => bail!(
            "{}: no feeder has enrolled yet; start serve with --enroll-minutes during the install",
            path.display()
        ),
    }
}

/// The config at `path`, and whether it has a device yet.
pub fn state(path: &Path) -> Result<State> {
    let bytes = fs::read(path).context("read configuration")?;
    let enrollment = Enrollment::load(&Enrollment::path(path))?;
    let state = parse_state(&bytes, enrollment).context("invalid server configuration")?;
    match &state {
        State::Device(config) => config.validate()?,
        State::Unenrolled(config) => config.validate()?,
    }
    Ok(state)
}

/// A config's fields with the device taken from `enrollment` when the file
/// has none of its own; both at once is refused, since which feeder the
/// server answers would then be a guess.
fn parse_state(bytes: &[u8], enrollment: Option<Enrollment>) -> Result<State> {
    let UniqueKeys(mut fields) = serde_json::from_slice(bytes)?;
    fields.remove(RETIRED_ROLE);

    match (fields.contains_key(DEVICE_FIELD), enrollment) {
        (true, None) => Ok(State::Device(Box::new(serde_json::from_value(
            Value::Object(fields),
        )?))),
        (true, Some(_)) => {
            bail!("the config names a captured device and a feeder enrolled beside it; keep one")
        }
        (false, None) => Ok(State::Unenrolled(serde_json::from_value(Value::Object(
            fields,
        ))?)),
        (false, Some(enrolled)) => {
            for (key, value) in [
                (DEVICE_FIELD, &enrolled.client_id),
                ("device_client_id", &enrolled.client_id),
                ("device_username", &enrolled.username),
                ("device_password", &enrolled.password),
            ] {
                fields.insert(key.into(), Value::String(value.clone()));
            }
            fields.insert(
                "device_address".into(),
                Value::String(enrolled.address.to_string()),
            );
            Ok(State::Device(Box::new(serde_json::from_value(
                Value::Object(fields),
            )?)))
        }
    }
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

    const INSTALLER: &str = r#"{"version":1,
        "server_password":"ssssssssssssssssssssssssssssssss",
        "api_token":"tttttttttttttttttttttttttttttttt",
        "mqtt_listen":"127.0.0.1:1883","http_listen":"127.0.0.1:8765",
        "timezone":"UTC"}"#;

    /// A config with its own device.
    fn parse(bytes: &[u8]) -> Result<Config> {
        match parse_state(bytes, None)? {
            State::Device(config) => Ok(*config),
            State::Unenrolled(_) => bail!("no device"),
        }
    }

    fn enrolled() -> Enrollment {
        Enrollment {
            client_id: "AF_NEW".into(),
            username: "stock".into(),
            password: "its-own".into(),
            address: "192.168.4.20".parse().unwrap(),
            enrolled_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn an_installer_s_config_takes_its_device_from_the_enrollment() {
        let State::Unenrolled(config) = parse_state(INSTALLER.as_bytes(), None).unwrap() else {
            panic!("no device before an enrollment");
        };
        assert!(config.validate().is_ok());

        let State::Device(config) = parse_state(INSTALLER.as_bytes(), Some(enrolled())).unwrap()
        else {
            panic!("the enrolled device");
        };
        assert!(config.validate().is_ok());
        assert_eq!(config.device_id, "AF_NEW");
        assert_eq!(config.device_client_id, "AF_NEW");
        assert_eq!(config.device_password, "its-own");
        assert_eq!(config.device_address, Some("192.168.4.20".parse().unwrap()));
        assert!(config.hotspot().is_err(), "no redirect for an installer");
        assert!(config.plans.is_empty());
    }

    #[test]
    fn a_captured_device_and_an_enrollment_together_are_refused() {
        let captured = format!("{{{STOCK}}}");
        assert!(parse_state(captured.as_bytes(), Some(enrolled())).is_err());
        let config = parse(captured.as_bytes()).unwrap();
        assert_eq!(config.hotspot().unwrap(), ("wlan1", "02:00:00:00:00:01"));
    }

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

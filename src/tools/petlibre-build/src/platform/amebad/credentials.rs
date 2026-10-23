//! The generated credential headers: FW07 Wi-Fi join and FW08 broker
//! settings, read from ignored private files into untracked SDK headers.
//!
//! Only the private files' hashes reach the manifest (they are identity
//! inputs). Values are octal-escaped C strings, so no SSID or password can
//! break out of its literal.

use petlibre_formats::credentials::{FirmwareCredential, WifiCredential};
use petlibre_formats::hex::is_lower_hex;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::{Error, Result, require};

const BANNER: &str = "/* Generated from a private file by petlibre-build; never tracked. */\n";

/// Octal-escape everything but plain printable ASCII; never a trigraph or
/// quote break.
fn c_string(value: &[u8]) -> String {
    let mut out = String::from("\"");
    for &byte in value {
        if (0x20..0x7f).contains(&byte) && !b"\"\\?".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("\\{byte:03o}"));
        }
    }
    out.push('"');
    out
}

/// A credential file, which must be an object: derived structs would also
/// accept their fields as an array.
fn object<T: DeserializeOwned>(raw: &[u8]) -> serde_json::Result<T> {
    let map: Map<String, Value> = serde_json::from_slice(raw)?;
    serde_json::from_value(Value::Object(map))
}

/// The Wi-Fi header; refuses anything but a WPA2-sized passphrase.
pub(crate) fn wifi_header(raw: &[u8]) -> Result<String> {
    let WifiCredential { ssid, password } = object(raw)
        .map_err(|_| Error::Refused("Wi-Fi credentials need string ssid and password".into()))?;
    require(
        (1..=32).contains(&ssid.len()) && (8..=63).contains(&password.len()),
        "Wi-Fi credentials need a 1-32 byte SSID and an 8-63 byte passphrase",
    )?;

    Ok(format!(
        "{BANNER}#define PETLIBRE_WIFI_SSID {}\n#define PETLIBRE_WIFI_PASSWORD {}\n",
        c_string(ssid.as_bytes()),
        c_string(password.as_bytes())
    ))
}

/// The broker settings header; refuses anything `petlibre-local
/// firmware-credential` would not have written.
pub(crate) fn transport_header(raw: &[u8]) -> Result<String> {
    let config: FirmwareCredential = object(raw).map_err(|error| {
        Error::Refused(format!(
            "Transport credentials need exactly the version 1 fields: {error}"
        ))
    })?;
    require(
        config.version == 1,
        "Transport credentials need exactly the version 1 fields",
    )?;

    let request_key = config.request_key.as_deref().unwrap_or_default();
    require(
        request_key.is_empty() || is_lower_hex(request_key, 64),
        "Transport request key must be 64 lowercase hex characters",
    )?;

    let device = &config.device_id;
    require(
        (1..=64).contains(&device.len())
            && device
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
        "Transport credentials need a plain device ID",
    )?;

    let broker = config.broker;
    require(
        !broker.is_unspecified() && !broker.is_multicast() && !broker.is_broadcast(),
        "Transport broker must be a unicast IPv4 address",
    )?;
    require(config.port != 0, "Transport port out of range")?;
    require(
        config.client_id == format!("petlibre-fw-{device}")
            && config.username == "petlibre-fw"
            && config.namespace == format!("petlibre/1/{device}/"),
        "Transport identity does not match the device ID",
    )?;

    let password = &config.password;
    require(
        (32..=128).contains(&password.len()) && is_lower_hex(password, password.len()),
        "Transport password must be 32-128 lowercase hex characters",
    )?;

    let mut out = String::from(BANNER);
    let mut define = |key: &str, value: String| {
        out.push_str(&format!("#define PETLIBRE_MQTT_{key} {value}\n"));
    };
    define("BROKER", format!("0x{:08x}U", u32::from(broker)));
    define("PORT", format!("{}U", config.port));
    for (key, value) in [
        ("CLIENT_ID", &config.client_id),
        ("USERNAME", &config.username),
        ("PASSWORD", &config.password),
        ("NAMESPACE", &config.namespace),
    ] {
        define(key, c_string(value.as_bytes()));
    }
    define("REQUEST_KEY", c_string(request_key.as_bytes()));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn good() -> Value {
        json!({"version": 1, "broker": "10.42.103.1", "port": 1883, "device_id": "dev1",
               "client_id": "petlibre-fw-dev1", "username": "petlibre-fw",
               "password": "0123456789abcdef".repeat(4), "namespace": "petlibre/1/dev1/"})
    }

    fn header(change: Value) -> Result<String> {
        let mut config = good();
        for (key, value) in change.as_object().unwrap() {
            config[key] = value.clone();
        }
        transport_header(config.to_string().as_bytes())
    }

    #[test]
    fn good_credentials_become_defines() {
        let text = header(json!({})).unwrap();
        assert!(text.contains("#define PETLIBRE_MQTT_BROKER 0x0a2a6701U\n"));
        assert!(text.contains("#define PETLIBRE_MQTT_PORT 1883U\n"));
        assert!(text.contains("#define PETLIBRE_MQTT_CLIENT_ID \"petlibre-fw-dev1\"\n"));
        assert!(text.contains("#define PETLIBRE_MQTT_NAMESPACE \"petlibre/1/dev1/\"\n"));
        assert!(text.contains(&format!(
            "#define PETLIBRE_MQTT_PASSWORD \"{}\"\n",
            "0123456789abcdef".repeat(4)
        )));
        assert!(text.contains("#define PETLIBRE_MQTT_REQUEST_KEY \"\"\n"));

        let keyed = header(json!({"request_key": "ab".repeat(32)})).unwrap();
        assert!(keyed.contains(&format!("REQUEST_KEY \"{}\"", "ab".repeat(32))));
    }

    #[test]
    fn anything_the_server_would_not_write_is_refused() {
        for change in [
            json!({"version": 2}),
            json!({"extra": 1}),
            json!({"broker": "0.0.0.0"}),
            json!({"broker": "224.0.0.1"}),
            json!({"broker": "255.255.255.255"}),
            json!({"port": 0}),
            json!({"port": "1883"}),
            json!({"port": true}),
            json!({"client_id": "other"}),
            json!({"username": "device"}),
            json!({"namespace": "dl/PLAF103/dev1/"}),
            json!({"device_id": "dev/1"}),
            json!({"password": "short"}),
            json!({"password": "X".repeat(64)}),
            json!({"password": format!("\"{}", "a".repeat(63))}),
            json!({"request_key": "AB".repeat(32)}),
        ] {
            assert!(header(change.clone()).is_err(), "{change}");
        }

        let mut missing = good();
        missing.as_object_mut().unwrap().remove("port");
        assert!(transport_header(missing.to_string().as_bytes()).is_err());
    }

    #[test]
    fn wifi_values_are_escaped_and_sized() {
        let text = wifi_header(br#"{"ssid": "lab\"?", "password": "pass\\word"}"#).unwrap();
        assert!(text.contains("#define PETLIBRE_WIFI_SSID \"lab\\042\\077\"\n"));
        assert!(text.contains("#define PETLIBRE_WIFI_PASSWORD \"pass\\134word\"\n"));

        for bad in [
            r#"{"ssid": "", "password": "longenough"}"#,
            r#"{"ssid": "lab", "password": "short"}"#,
            r#"{"ssid": 1, "password": "longenough"}"#,
            r#"["lab", "longenough"]"#,
        ] {
            assert!(wifi_header(bad.as_bytes()).is_err(), "{bad}");
        }
    }
}
